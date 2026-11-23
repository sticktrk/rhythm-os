import assert from "node:assert/strict";
import { test } from "node:test";
import { validateSteps, type Turn } from "../src/planner.ts";

const room = "r_" + "1".repeat(24),
  sensor = "m_" + "2".repeat(24),
  scene = "c_" + "3".repeat(24),
  schedule = "s_" + "4".repeat(24);
const turn: Turn = {
  turn: crypto.randomUUID(),
  messages: [{ role: "user", text: "Help with my lights" }],
  targets: [
    { ref: room, tools: [], included: true },
    { ref: sensor, tools: ["start_motion_walk", "plan_motion_target"] },
    { ref: scene, tools: ["plan_home_scene_edit"] },
    { ref: schedule, tools: ["plan_light_schedule_edit"] },
  ],
};
const step = (
  target: string,
  operation: string,
  args: Record<string, number | string>,
) => ({ target, operation, arguments: args });

test("motion repair requires a walk that identified the missing intended target", () => {
  const repair = step(sensor, "plan_motion_target", { room });
  assert.throws(() => validateSteps([repair], turn));
  const walked = {
    ...turn,
    motion: {
      sensor,
      room,
      detected: true,
      missing_target: true,
      stage: "detected" as const,
      fresh: 0,
    },
  };
  assert.deepEqual(validateSteps([repair], walked), [repair]);
  assert.throws(() =>
    validateSteps([repair], {
      ...walked,
      targets: walked.targets.map((t) => ({ ...t, included: false })),
    }),
  );
  assert.throws(() =>
    validateSteps([repair], {
      ...walked,
      motion: { ...walked.motion, missing_target: false },
    }),
  );
});
test("saved settings require a named, offered capability and a bounded edit", () => {
  const shift = step(schedule, "plan_light_schedule_edit", {
    delta_minutes: -15,
    transition: "sleep",
  });
  assert.deepEqual(validateSteps([shift], turn), [shift]);
  assert.throws(() =>
    validateSteps(
      [{ ...shift, arguments: { delta_minutes: -800, transition: "all" } }],
      turn,
    ),
  );
  assert.throws(() =>
    validateSteps([shift, step(scene, "plan_home_scene_edit", {})], turn),
  );
  assert.throws(() =>
    validateSteps([step(room, "plan_home_scene_edit", {})], turn),
  );
});
