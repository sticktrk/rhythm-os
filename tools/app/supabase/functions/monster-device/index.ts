import { corsHeaders, withAuthenticatedRequest } from "../_shared/auth.ts";
import { createMonsterBroker } from "./broker.ts";
const broker = createMonsterBroker({
  env: (name) => Deno.env.get(name),
  fetch,
});
Deno.serve((req) =>
  withAuthenticatedRequest(req, async ({ userId }) => {
    const result = await broker(req, userId);
    for (const [key, value] of Object.entries(corsHeaders)) {
      result.headers.set(key, value);
    }
    // Status/stage only; never request bodies, vendor messages, account IDs or keys.
    console.info(
      JSON.stringify({
        event: "monster_broker_outcome",
        status: result.status,
      }),
    );
    return result;
  })
);
