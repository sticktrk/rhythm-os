export type CurvePoints = {
  brightness: Array<{ hour: number; value: number }>;
  kelvin: Array<{ hour: number; value: number }>;
};

function recordOf(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function numberOf(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function arrayOf(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

/** Parse the appliance's `CurveResponse`, while retaining support for the
    earlier flat sample payload accepted by the admin UI. */
export function parseCurveSamples(payload: unknown): CurvePoints {
  const response = recordOf(payload);
  const nestedCurve = recordOf(response.curve);
  const record = Object.keys(nestedCurve).length > 0 ? nestedCurve : response;
  const hours = arrayOf(record.hours).map(numberOf);
  const brightnessArr = arrayOf(record.brightness).map(numberOf);
  const kelvinArr = arrayOf(record.kelvin ?? record.color_temp).map(numberOf);

  if (hours.length > 0) {
    const brightness: CurvePoints['brightness'] = [];
    const kelvin: CurvePoints['kelvin'] = [];
    hours.forEach((hour, index) => {
      if (hour === undefined) return;
      const brightnessValue = brightnessArr[index];
      const kelvinValue = kelvinArr[index];
      if (brightnessValue !== undefined) {
        brightness.push({ hour, value: brightnessValue });
      }
      if (kelvinValue !== undefined) {
        kelvin.push({ hour, value: kelvinValue });
      }
    });
    return { brightness, kelvin };
  }

  const points = arrayOf(record.points ?? record.samples ?? record.data ?? payload)
    .map(recordOf);
  const brightness: CurvePoints['brightness'] = [];
  const kelvin: CurvePoints['kelvin'] = [];
  for (const point of points) {
    const hour = numberOf(point.hour) ?? numberOf(point.h);
    if (hour === undefined) continue;
    const brightnessValue = numberOf(point.brightness) ?? numberOf(point.bri);
    const kelvinValue =
      numberOf(point.kelvin) ??
      numberOf(point.color_temp) ??
      numberOf(point.cct);
    if (brightnessValue !== undefined) {
      brightness.push({ hour, value: brightnessValue });
    }
    if (kelvinValue !== undefined) {
      kelvin.push({ hour, value: kelvinValue });
    }
  }
  return { brightness, kelvin };
}
