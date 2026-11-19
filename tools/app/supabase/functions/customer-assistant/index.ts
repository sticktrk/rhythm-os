import { handleRequest } from "./handler.ts";

Deno.serve((req) => handleRequest(req));
