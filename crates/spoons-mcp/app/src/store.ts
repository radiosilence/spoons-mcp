// The app's state, and the things a tap can do. Every action is GraphQL through the `spoons` tool, and every
// change the user makes is told to the model.

import { useEffect, useState } from "preact/hooks";
import { gql, say, tellModel } from "./bridge";
import * as Q from "./queries";
import type { Basket, Hints, Portion, Product, Quote, View } from "./types";

export interface State {
  /** What the latest tool result asked to show; `null` shows nothing. */
  view: View | null;
  hints: Hints;
  compact: boolean;
  basket: Basket | null;
  busy: boolean;
  toast: { text: string; tone: "ok" | "bad" } | null;
  /** Screens the user has moved through in the app, for the back button. */
  history: View[];
}

let state: State = {
  view: null,
  hints: {},
  compact: false,
  basket: null,
  busy: false,
  toast: null,
  history: [],
};

const listeners = new Set<() => void>();

export function get(): State {
  return state;
}

export function set(patch: Partial<State>) {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
}

export function useStore(): State {
  const [, force] = useState(0);
  useEffect(() => {
    const l = () => force((n) => n + 1);
    listeners.add(l);
    return () => void listeners.delete(l);
  }, []);
  return state;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function toast(text: string, tone: "ok" | "bad" = "ok") {
  set({ toast: { text, tone } });
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => set({ toast: null }), 3200);
}

/** Moves to a screen the user chose, remembering where they were. */
export function go(view: View) {
  const { view: current, history } = state;
  set({ view, history: current ? [...history, current].slice(-8) : history, compact: false });
}

export function back() {
  const history = [...state.history];
  const previous = history.pop();
  if (previous) set({ view: previous, history });
}

function newBasket(basket: Basket) {
  if (!state.basket || basket.version >= state.basket.version) set({ basket });
}

type Result = { success: boolean; error: string | null; basket: Basket };

async function change(op: string, variables: Record<string, unknown>, said: (b: Basket) => string) {
  set({ busy: true });
  try {
    const data = await gql<Record<string, Result>>(op, variables);
    const result = Object.values(data)[0];
    newBasket(result.basket);
    if (!result.success) {
      toast(result.error ?? "That didn’t work.", "bad");
      return false;
    }
    void tellModel(`In the Wetherspoon's app, the user ${said(result.basket)} Basket now: ${result.basket.summary}`, {
      basket: result.basket,
    });
    return true;
  } catch (e) {
    toast(String((e as Error).message ?? e), "bad");
    return false;
  } finally {
    set({ busy: false });
  }
}

export async function add(pub: number, product: Product, portion: Portion) {
  const ok = await change(Q.ADD, { pub, product: product.id, portion: portion.name }, () =>
    `added 1 × ${product.name} (${portion.name}).`,
  );
  if (ok) toast(`${product.shortName} · ${portion.name} added`);
}

export async function setQuantity(lineId: string, name: string, quantity: number) {
  await change(Q.SET, { line: lineId, q: Math.max(0, quantity) }, () =>
    quantity <= 0 ? `removed ${name}.` : `changed ${name} to ${quantity}.`,
  );
}

export async function setTable(table: number, pub: number | null) {
  const ok = await change(Q.SET_TABLE, { table, pub }, () => `set their table to ${table}.`);
  if (ok) toast(`Table ${table}`);
}

export async function clear() {
  await change(Q.CLEAR, {}, () => "emptied the basket.");
}

/** Prices the basket at the till. Changes nothing at the pub. */
export async function quote(): Promise<Quote | null> {
  set({ busy: true });
  try {
    const { quote } = await gql<{ quote: { success: boolean; error: string | null; quote: Quote | null } }>(Q.MAKE_QUOTE);
    if (!quote.success || !quote.quote) {
      toast(quote.error ?? "The till didn’t price it.", "bad");
      return null;
    }
    void tellModel(
      `In the Wetherspoon's app, the user asked the till for a quote: ${quote.quote.summary} (quote ${quote.quote.id}). ` +
        `Nothing is paid; paying needs pay PREVIEW and the user's explicit yes in the chat.`,
      { quote: quote.quote },
    );
    go({ screen: "quote", quoteId: quote.quote.id });
    return quote.quote;
  } catch (e) {
    toast(String((e as Error).message ?? e), "bad");
    return null;
  } finally {
    set({ busy: false });
  }
}

/** Hands the decision to the conversation, as the user's own message. The app never pays. */
export async function askToPay(q: Quote) {
  const sent = await say(
    `Quote ${q.id} is ready: ${q.summary} Can you show me the payment preview? I'll confirm in the chat.`,
  );
  toast(sent ? "Sent to the chat" : "Ask in the chat to pay", sent ? "ok" : "bad");
}

/** Whether this app is in the viewport. Many may be in a long conversation; only the ones in view keep watching,
 * so they don't hold the host's connections to the server open. */
let onScreen = true;
const seen = new IntersectionObserver((entries) => {
  onScreen = entries.some((e) => e.isIntersecting);
});
seen.observe(document.documentElement);

/** Keeps the basket current while the app is on screen, whoever changes it. */
export async function watchBasket(signal: AbortSignal) {
  try {
    const { basket } = await gql<{ basket: Basket }>(Q.BASKET);
    newBasket(basket);
  } catch {
    /* the loop tries again */
  }
  let failures = 0;
  while (!signal.aborted) {
    if (document.hidden || !onScreen) {
      await new Promise((r) => setTimeout(r, 1500));
      continue;
    }
    try {
      const { basket } = await gql<{ basket: Basket }>(Q.BASKET_WAIT, { v: state.basket?.version ?? 0 });
      newBasket(basket);
      failures = 0;
    } catch {
      failures++;
      await new Promise((r) => setTimeout(r, Math.min(30_000, 1000 * 2 ** failures)));
    }
  }
}
