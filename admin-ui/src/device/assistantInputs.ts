type Fields = Record<string, unknown>;

export function argumentTypes(schema: Fields): string[] {
  const alternatives = Array.isArray(schema.anyOf) ? schema.anyOf : schema.oneOf;
  if (Array.isArray(alternatives)) {
    return alternatives.flatMap(s => s && typeof s === 'object' ? argumentTypes(s as Fields) : []);
  }
  return typeof schema.type === 'string' ? [schema.type] :
    Array.isArray(schema.type) ? schema.type.filter((type): type is string => typeof type === 'string') : [];
}

export function assistantArguments(schema: Fields, fields: Record<string, string>, correlation: () => string): Fields {
  const args: Fields = {};
  const required = Array.isArray(schema.required) ? schema.required : [];
  for (const [key, def] of Object.entries((schema.properties ?? {}) as Fields)) {
    const definition = def as Fields;
    const types = argumentTypes(definition);
    const value = fields[key]?.trim() ?? '';
    if (!value) {
      if (key === 'correlation_id' && required.includes(key)) args[key] = correlation();
      else if (required.includes(key)) throw new Error(`Enter ${key.replaceAll('_', ' ')}.`);
      continue;
    }
    if (Array.isArray(definition.enum)) {
      const option = definition.enum.find(candidate => String(candidate) === value);
      if (option === undefined) throw new Error(`Choose an advertised value for ${key}.`);
      args[key] = option;
    } else if (types.includes('string') && !(types.includes('null') && value === 'null')) {
      args[key] = value;
    } else if (types.length) {
      let parsed: unknown;
      try { parsed = JSON.parse(value); } catch { throw new Error(`Enter a valid ${types.join(' or ')} for ${key}.`); }
      const type = parsed === null ? 'null' : Array.isArray(parsed) ? 'array' : typeof parsed;
      if (!types.includes(type) && !(types.includes('integer') && Number.isInteger(parsed))) {
        throw new Error(`Enter a valid ${types.join(' or ')} for ${key}.`);
      }
      args[key] = parsed;
    } else {
      args[key] = value;
    }
  }
  return args;
}

export function walkReports(
  previous: Record<string, boolean>, update?: { device_id: string; power_on: boolean }
) {
  const reports = update ? { ...previous, [update.device_id]: update.power_on } : previous;
  return {
    reports,
    observations: Object.entries(reports).map(([device_id, power_on]) => ({ device_id, power_on }))
  };
}
