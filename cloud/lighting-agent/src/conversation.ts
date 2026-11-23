import { Agent } from "agents";
import { DurableObject } from "cloudflare:workers";
import { z } from "zod";
import { id, type Step } from "./planner.ts";
import type { Env } from "./index.ts";

export const week = 7 * 86400_000;
const reviewLife = 5 * 60_000;
const outcomes = z.enum([
  "acknowledged",
  "observed",
  "verified",
  "awaiting_owner",
  "uncertain",
  "rejected",
  "walk_started",
  "partial",
  "queued",
]);
export type Outcome = z.infer<typeof outcomes>;
export type Progress = Step & {
  status: "review" | "claimed" | "cancelled" | Outcome;
  nonce?: string;
  receipt?: string;
  digest?: string;
};
export interface Plan {
  id: string;
  expires: number;
  steps: Progress[];
}
type LastResult = "photo_advice" | "advice" | "review";
interface Snapshot {
  history?: Plan[];
  lastResult?: LastResult;
  session: string;
  revision: number;
  touched: number;
  plan: Plan | null;
  turns: { id: string; at: number }[];
  active: string | null;
}
type Data = {
  history?: Plan[];
  lastResult?: LastResult;
  session?: string;
  revision?: number;
  plan?: Plan | null;
  reason?: string;
  cleared?: boolean;
  claim?: string;
};
type Result = { code: number; data: Data };
const result = (code: number, data: Data = {}): Result => ({ code, data });
const opaque = z.string().regex(/^[a-zA-Z0-9_-]{1,120}$/);

export class LightingConversation extends Agent<Env, Snapshot | null> {
  initialState = null;
  private snapshot(s: Snapshot) {
    const visible = (p: Plan) => ({
      ...p,
      steps: p.steps.map(({ nonce, ...rest }) => rest),
    });
    return {
      session: s.session,
      revision: s.revision,
      lastResult: s.lastResult,
      history: (s.history ?? []).map(visible),
      plan: s.plan && visible(s.plan),
    };
  }
  private commit(s: Snapshot) {
    this.setState({ ...s, touched: Date.now(), revision: s.revision + 1 });
    return result(200, this.snapshot(this.state!));
  }
  async open(): Promise<Result> {
    if (!this.state || this.state.touched + week <= Date.now()) {
      await this.schedule(week / 1000, "expire", undefined, { idempotent: true });
      if (!this.state || this.state.touched + week <= Date.now())
        this.setState({
          session: crypto.randomUUID(),
          touched: Date.now(),
          revision: 0,
          plan: null,
          active: null,
          turns: [],
        });
    }
    return result(200, this.snapshot(this.state!));
  }
  erase() {
    // Inline destruction can abort the RPC before its acknowledgement reaches the Worker.
    // Erase the snapshot now. The existing expiry alarm reclaims the empty object.
    this.setState(null);
  }
  async expire() {
    const remaining = (this.state?.touched ?? 0) + week - Date.now();
    if (remaining <= 0) await this.destroy();
    else
      await this.schedule(Math.max(1, Math.ceil(remaining / 1000)), "expire");
  }
  async control(
    session: string,
    action: string,
    body: Record<string, unknown>,
  ): Promise<Result> {
    const current = this.state;
    if (
      !current ||
      current.session !== session ||
      current.touched + week <= Date.now()
    )
      return result(410);
    const s = structuredClone(current);
    if (action === "status") return result(200, this.snapshot(s));
    if (action === "clear") {
      this.erase();
      return result(200, { cleared: true });
    }
    if (action === "begin") {
      if (!id.safeParse(body.turn).success) return result(400);
      if (s.turns.some((t) => t.id === body.turn))
        return result(409, {
          ...this.snapshot(s),
          reason: "turn_already_received",
        });
      const now = Date.now();
      if (s.active && s.touched + 60_000 > now)
        return result(409, { reason: "turn_in_progress" });
      if (
        s.plan &&
        s.plan.expires > now &&
        s.plan.steps.some((step) => step.status === "review")
      )
        return result(409, { reason: "review_pending" });
      s.turns = s.turns.filter((t) => t.at + week > now);
      if (s.turns.length >= 480) return result(429);
      s.active = body.turn as string;
      s.turns.push({ id: s.active, at: now });
      return this.commit(s);
    }
    if (action === "abort") {
      if (s.active === body.turn) {
        s.active = null;
        return this.commit(s);
      }
      return result(409);
    }
    const plan =
      action === "receipt"
        ? [s.plan, ...(s.history ?? [])].find((p) => p?.id === body.plan)
        : s.plan;
    if (!plan || plan.id !== body.plan)
      return result(409, { reason: "plan_changed" });
    if (action === "cancel") {
      for (const step of plan.steps)
        if (step.status === "review") step.status = "cancelled";
      return this.commit(s);
    }
    const i = body.index;
    if (
      typeof i !== "number" ||
      !Number.isInteger(i) ||
      i < 0 ||
      i >= plan.steps.length
    )
      return result(400);
    const step = plan.steps[i];
    if (action === "prepare") {
      if (
        step.status !== "review" ||
        plan.expires <= Date.now() ||
        typeof body.digest !== "string" ||
        !/^[a-f0-9]{64}$/.test(body.digest)
      )
        return result(409);
      if (step.digest && step.digest !== body.digest)
        return result(409, { reason: "review_changed" });
      step.digest = body.digest;
      return this.commit(s);
    }
    if (action === "claim") {
      if (
        !step.digest ||
        step.digest !== body.digest ||
        plan.expires <= Date.now() ||
        step.status !== "review"
      )
        return result(409, this.snapshot(s));
      step.status = "claimed";
      step.nonce = crypto.randomUUID();
      const applied = this.commit(s);
      return result(200, { ...applied.data, claim: step.nonce });
    }
    if (action === "receipt") {
      const parsed = outcomes.safeParse(body.outcome);
      if (
        !parsed.success ||
        typeof body.claim !== "string" ||
        step.nonce !== body.claim
      )
        return result(409);
      if (step.status !== "claimed") return result(200, this.snapshot(s));
      if (body.receipt !== undefined && !opaque.safeParse(body.receipt).success)
        return result(400);
      step.status = parsed.data;
      if (typeof body.receipt === "string") step.receipt = body.receipt;
      return this.commit(s);
    }
    return result(404);
  }
  finish(session: string, turn: string, steps: Step[], photo: boolean): Result {
    const current = this.state;
    if (!current || session !== current.session || current.active !== turn)
      return result(409);
    const s = structuredClone(current);
    s.active = null;
    s.lastResult = steps.length ? "review" : photo ? "photo_advice" : "advice";
    if (steps.length) {
      const retained = [
        ...(s.history ?? []),
        ...(s.plan ? [s.plan] : []),
      ].filter((p) => p.expires + week > Date.now());
      // Late receipts and active walks stay addressable after a different request.
      if (retained.length > 24) {
        const disposable = retained.findIndex((p) =>
          p.steps.every(
            (st) =>
              ![
                "claimed",
                "awaiting_owner",
                "uncertain",
                "walk_started",
              ].includes(st.status),
          ),
        );
        if (disposable < 0) {
          this.commit(s);
          return result(409, { reason: "review_previous_outcomes" });
        }
        retained.splice(disposable, 1);
      }
      s.history = retained;
      s.plan = {
        id: crypto.randomUUID(),
        expires: Date.now() + reviewLife,
        steps: steps.map((step) => ({ ...step, status: "review" })),
      };
    }
    return this.commit(s);
  }
}

// Account deletion can durably enqueue erasure independently of individual phones.
export class ConversationIndex extends DurableObject<Env> {
  async register(name: string) {
    if (await this.ctx.storage.get("forget")) return false;
    const db = this.ctx.storage.sql;
    db.exec(
      "CREATE TABLE IF NOT EXISTS threads(name TEXT PRIMARY KEY, touched INTEGER NOT NULL)",
    );
    db.exec("DELETE FROM threads WHERE touched < ?", Date.now() - week);
    db.exec("INSERT OR REPLACE INTO threads VALUES (?,?)", name, Date.now());
    if ((await this.ctx.storage.getAlarm()) === null)
      await this.ctx.storage.setAlarm(Date.now() + week);
    return true;
  }
  async forget() {
    await this.ctx.storage.put("forget", Date.now() + week);
    await this.ctx.storage.setAlarm(Date.now() + 1);
  }
  async alarm() {
    const db = this.ctx.storage.sql;
    const purgeUntil = await this.ctx.storage.get<number>("forget");
    if (!purgeUntil) {
      db.exec(
        "CREATE TABLE IF NOT EXISTS threads(name TEXT PRIMARY KEY, touched INTEGER NOT NULL)",
      );
      db.exec("DELETE FROM threads WHERE touched < ?", Date.now() - week);
      if (db.exec("SELECT name FROM threads LIMIT 1").toArray().length)
        await this.ctx.storage.setAlarm(Date.now() + week);
      else await this.ctx.storage.deleteAll();
      return;
    }
    db.exec(
      "CREATE TABLE IF NOT EXISTS threads(name TEXT PRIMARY KEY, touched INTEGER NOT NULL)",
    );
    for (const { name } of db
      .exec<{ name: string }>("SELECT name FROM threads")
      .toArray()) {
      await this.env.CONVERSATIONS.getByName(name).erase();
      db.exec("DELETE FROM threads WHERE name=?", name);
    }
    // Keep the account closed while deletion and any in-flight requests settle.
    if (purgeUntil > Date.now()) await this.ctx.storage.setAlarm(purgeUntil);
    else await this.ctx.storage.deleteAll();
  }
}
