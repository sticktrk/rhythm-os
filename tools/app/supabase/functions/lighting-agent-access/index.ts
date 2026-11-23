import { access } from "./access.ts";

Deno.serve((req) => access(req));
