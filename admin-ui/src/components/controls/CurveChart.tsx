import type { ReactNode } from 'react';

import { formatHour } from '../../lib/format';

export type CurveSeries = {
  id: string;
  label: string;
  axis: 'brightness' | 'kelvin';
  color: string;
  points: Array<{ hour: number; value: number }>;
};

export type CurveChartProps = {
  series: CurveSeries[];
  height?: number;
  nowHour?: number;
  solar?: { sunriseHour?: number; sunsetHour?: number };
  yLeft?: { min: number; max: number };
  yRight?: { min: number; max: number };
  xAxisLabel?: string;
  yLeftAxisLabel?: string;
  yRightAxisLabel?: string;
  /** Extra SVG rendered in plot coordinates (used by CurveEditor handles). */
  overlay?: ReactNode;
  onPlotGeometry?: (geometry: PlotGeometry) => void;
};

export type PlotGeometry = {
  width: number;
  height: number;
  padding: { left: number; right: number; top: number; bottom: number };
  xForHour: (hour: number) => number;
  hourForX: (x: number) => number;
  yForValue: (value: number, axis: 'brightness' | 'kelvin') => number;
  valueForY: (y: number, axis: 'brightness' | 'kelvin') => number;
};

export const CURVE_CHART_WIDTH = 760;

export function plotGeometry(
  height: number,
  yLeft: { min: number; max: number },
  yRight: { min: number; max: number }
): PlotGeometry {
  const padding = { left: 44, right: 52, top: 14, bottom: 26 };
  const plotWidth = CURVE_CHART_WIDTH - padding.left - padding.right;
  const plotHeight = height - padding.top - padding.bottom;

  const xForHour = (hour: number) =>
    padding.left + (Math.min(24, Math.max(0, hour)) / 24) * plotWidth;
  const hourForX = (x: number) =>
    Math.min(24, Math.max(0, ((x - padding.left) / plotWidth) * 24));
  const yForValue = (value: number, axis: 'brightness' | 'kelvin') => {
    const range = axis === 'brightness' ? yLeft : yRight;
    const t =
      range.max > range.min
        ? (value - range.min) / (range.max - range.min)
        : 0;
    return padding.top + (1 - Math.min(1, Math.max(0, t))) * plotHeight;
  };
  const valueForY = (y: number, axis: 'brightness' | 'kelvin') => {
    const range = axis === 'brightness' ? yLeft : yRight;
    const t = 1 - (y - padding.top) / plotHeight;
    return range.min + Math.min(1, Math.max(0, t)) * (range.max - range.min);
  };

  return {
    width: CURVE_CHART_WIDTH,
    height,
    padding,
    xForHour,
    hourForX,
    yForValue,
    valueForY
  };
}

function seriesPath(
  series: CurveSeries,
  geometry: PlotGeometry
): string {
  const points = [...series.points].sort((a, b) => a.hour - b.hour);
  return points
    .map((point, index) => {
      const x = geometry.xForHour(point.hour);
      const y = geometry.yForValue(point.value, series.axis);
      return `${index === 0 ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(' ');
}

/** Dual-axis 24h curve chart (SVG). Brightness on the left axis,
    kelvin on the right. Values must come from the device's curve sampler. */
export function CurveChart({
  series,
  height = 260,
  nowHour,
  solar,
  yLeft = { min: 0, max: 100 },
  yRight = { min: 500, max: 6500 },
  xAxisLabel,
  yLeftAxisLabel,
  yRightAxisLabel,
  overlay
}: CurveChartProps) {
  const geometry = plotGeometry(height, yLeft, yRight);
  const { padding } = geometry;
  const plotBottom = height - padding.bottom;

  const hourTicks = [0, 3, 6, 9, 12, 15, 18, 21, 24];
  const leftTicks = 4;
  const rightTicks = 4;

  return (
    <svg
      className="curveChart"
      viewBox={`0 0 ${CURVE_CHART_WIDTH} ${height}`}
      role="img"
      aria-label="24 hour lighting curve"
    >
      {/* grid */}
      {hourTicks.map((hour) => {
        const x = geometry.xForHour(hour);
        return (
          <g key={`h${hour}`}>
            <line
              className="curveGrid"
              x1={x}
              y1={padding.top}
              x2={x}
              y2={plotBottom}
            />
            <text
              className="curveTick"
              x={x}
              y={xAxisLabel ? height - 17 : height - 8}
              textAnchor="middle"
            >
              {formatHour(hour)}
            </text>
          </g>
        );
      })}
      {Array.from({ length: leftTicks + 1 }, (_, i) => {
        const value = yLeft.min + ((yLeft.max - yLeft.min) * i) / leftTicks;
        const y = geometry.yForValue(value, 'brightness');
        return (
          <g key={`l${i}`}>
            <line
              className="curveGrid"
              x1={padding.left}
              y1={y}
              x2={CURVE_CHART_WIDTH - padding.right}
              y2={y}
            />
            <text
              className="curveTick left"
              x={padding.left - 6}
              y={y + 3}
              textAnchor="end"
            >
              {Math.round(value)}
            </text>
          </g>
        );
      })}
      {Array.from({ length: rightTicks + 1 }, (_, i) => {
        const value = yRight.min + ((yRight.max - yRight.min) * i) / rightTicks;
        const y = geometry.yForValue(value, 'kelvin');
        return (
          <text
            key={`r${i}`}
            className="curveTick right"
            x={CURVE_CHART_WIDTH - padding.right + 6}
            y={y + 3}
            textAnchor="start"
          >
            {Math.round(value)}
          </text>
        );
      })}

      {xAxisLabel ? (
        <text
          className="curveAxisLabel"
          x={(padding.left + CURVE_CHART_WIDTH - padding.right) / 2}
          y={height - 2}
          textAnchor="middle"
        >
          {xAxisLabel}
        </text>
      ) : null}
      {yLeftAxisLabel ? (
        <text
          className="curveAxisLabel"
          textAnchor="middle"
          transform={`translate(10 ${(padding.top + plotBottom) / 2}) rotate(-90)`}
        >
          {yLeftAxisLabel}
        </text>
      ) : null}
      {yRightAxisLabel ? (
        <text
          className="curveAxisLabel"
          textAnchor="middle"
          transform={`translate(${CURVE_CHART_WIDTH - 8} ${(padding.top + plotBottom) / 2}) rotate(90)`}
        >
          {yRightAxisLabel}
        </text>
      ) : null}

      {/* solar markers */}
      {solar?.sunriseHour !== undefined ? (
        <SolarMarker
          x={geometry.xForHour(solar.sunriseHour)}
          top={padding.top}
          bottom={plotBottom}
          label="sunrise"
        />
      ) : null}
      {solar?.sunsetHour !== undefined ? (
        <SolarMarker
          x={geometry.xForHour(solar.sunsetHour)}
          top={padding.top}
          bottom={plotBottom}
          label="sunset"
        />
      ) : null}

      {/* now marker */}
      {nowHour !== undefined ? (
        <line
          className="curveNow"
          x1={geometry.xForHour(nowHour)}
          y1={padding.top}
          x2={geometry.xForHour(nowHour)}
          y2={plotBottom}
        />
      ) : null}

      {/* series */}
      {series.map((entry) =>
        entry.points.length > 1 ? (
          <path
            key={entry.id}
            className="curveSeries"
            d={seriesPath(entry, geometry)}
            stroke={entry.color}
          />
        ) : null
      )}

      {/* legend */}
      {series.map((entry, index) => (
        <g
          key={`legend-${entry.id}`}
          transform={`translate(${padding.left + index * 130}, ${padding.top - 2})`}
        >
          <rect
            className="curveLegendSwatch"
            width={10}
            height={3}
            y={-3}
            fill={entry.color}
          />
          <text className="curveLegend" x={14} y={0}>
            {entry.label}
          </text>
        </g>
      ))}

      {overlay}
    </svg>
  );
}

function SolarMarker({
  x,
  top,
  bottom,
  label
}: {
  x: number;
  top: number;
  bottom: number;
  label: string;
}) {
  return (
    <g>
      <line className="curveSolar" x1={x} y1={top} x2={x} y2={bottom} />
      <text className="curveSolarLabel" x={x + 3} y={top + 10}>
        {label}
      </text>
    </g>
  );
}
