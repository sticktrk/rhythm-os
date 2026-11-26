export const DEFAULT_LIGHTING_MODEL = "gpt-6.2-sol";

export function lightingModelSettings(override?: string) {
  const model = override?.trim() || DEFAULT_LIGHTING_MODEL;
  if (
    model === DEFAULT_LIGHTING_MODEL ||
    /^gpt-6\.2-sol-\d{4}-\d{2}-\d{2}$/.test(model)
  ) {
    return { model, reasoning: { effort: "low" as const } };
  }
  return { model };
}
