# Resident lighting agent

Flutter discovers this Worker through the authenticated `lighting-agent-access` Edge Function. The app resolves household names locally and the paired phone handles hub previews, approval, execution and readback. The Worker holds the public Supabase key. Supabase verifies the resident, the selected installation and membership on each request and reserves the existing daily lighting-chat allowance before inference.

The Cloudflare Agents SDK stores bounded progress in a SQLite Durable Object for each resident and installation. Clearing the conversation rotates its session and opaque target references. Concurrent phones share the same reviewed plan. Each step has one execution claim. A lost acknowledgement can be inspected from either phone. Read progress using `POST /v1/status` after reconnecting. Account deletion queues erasure through a resident-owned object index. Inactivity expires progress after seven days.

OpenAI GPT-6.2 Sol powers lighting conversations, room lookup, scene palettes and photo interpretation through the Responses API. The Worker sends recent messages and relevant lighting readings with `store: false`. For optional JPEGs sent to `POST /v2/photo`, it decodes and re-encodes the image before including it in the same request. The phone holds the transcript, and the conversation object stores structured outcomes. [OpenAI model details](https://developers.openai.com/api/docs/models/gpt-6.2-sol)

## Finding a room in conversation

Residents can name a room, describe it, or refer back to the room they just discussed. The phone replaces known names with opaque references. When both client and Worker support `room_lookup`, the model can return a short room description such as `kitchen` for “where I cook.” The phone checks that description against the hub’s rooms. A unique match gets a second inference turn with the room’s available operations and reading. Each inference uses one request from the chat allowance.

If a room is needed and the request is unclear, the phone asks for a name or number in chat and resumes the original request. The model’s lookup hint and the phone’s numbered room list stay out of durable progress. Whole-home and multi-room requests clear the previous room. The review shows the resolved target before a change can be applied.

## Creating a whole-home scene

Residents can describe a theme in chat or select **Make a scene**. A Worker that supports scene drafts advertises `scene_drafts` when opening a conversation. For a draft the phone sends the theme with one opaque design target. The model proposes two to four RGB colors and a brightness level. The phone shows swatches and prepares a hub review to save the palette as a new named scene.

Saving leaves the lights as they are. Once the phone verifies the saved definition on the hub, **Use across the home** prepares a separate review for the eligible rooms and their saved Mood bindings. Both steps honor the hub's owner-approval settings. A queued or partial dispatch is reported at that stage. Check outcome can recover a lost save response from the hub catalog.

## Development

Run these commands in this directory with Node 24 or newer.

```sh
npm ci
npm run check
npm test
npm run build
```

The Worker tests exercise real SQLite-backed objects in workerd, including a runtime restart. Account and lighting fixtures exercise authorization, quota, images and competing claims without credentials. Flutter coverage lives in `app/flutter/rhythm_app/test/screens/home_agent_chat_test.dart`.

## Deployment

Set `OPENAI_API_KEY` as a server secret on the Worker and in the Supabase project for room-only chat. Both runtimes default to `gpt-6.2-sol` with low reasoning effort. To override the model, set `RHYTHM_CHAT_MODEL` in both runtimes to an OpenAI Responses API model that supports image input, JSON output and function calling.

Deploy the named account bridge and customer assistant using the repository's Supabase deployment wrapper. Set Worker secrets `SUPABASE_URL` and `SUPABASE_PUBLISHABLE_KEY` to that project's URL and publishable key. Deploy using the local Wrangler dependency and the checked-in Durable Object migration. Set `RHYTHM_LIGHTING_AGENT_URL` in the same Supabase project to `https://api-cf.rhythm.lighting` for Rhythm. The account deletion function uses that setting for erasure.

The app opens `/v2/open` and verifies `inference_provider: openai` before enabling text or photo turns. An older app or Worker uses the room-only OpenAI chat when opening a conversation. Progress, approvals and receipts continue to use the existing `/v1` routes. If the Worker rolls back mid-conversation, the app can still reconcile a reviewed change, and the older Worker rejects incoming v2 inference requests.

Cloudflare handles lighting chat for all eligible signed-in residents by default. The mobile release build can set `RHYTHM_LIGHTING_AGENT_URL` to the same HTTPS origin to connect directly. Other installations discover their configured origin through the account bridge. `PILOT_RESIDENTS` accepts comma-separated authenticated resident UUIDs. `PILOT_PERCENT` enables a stable percentage of residents. Set `ACCEPT_NEW_TURNS=false` to pause inference and execution claims while existing sessions can reconcile receipts or clear their progress. Removing a resident from the cohort restores the previous mobile chat on the next open. Health checks use `GET /health`. Inspect the UTC quota and aggregate model usage when comparing pilot cost, and use bounded app outcome and latency events for completion and unresolved outcomes.
