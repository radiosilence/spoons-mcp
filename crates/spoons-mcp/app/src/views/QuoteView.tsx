import { useEffect, useState } from "preact/hooks";
import { gql, openLink } from "../bridge";
import * as Q from "../queries";
import { payNow } from "../store";
import type { Order, OrderStatus, PayLike, Quote } from "../types";
import { Icon, Pouring } from "../ui";

/** States the payment moves on from by itself. PREPARING isn't one: it has no payment to watch yet, and only stays
 * if a server died mid-request, so polling it would spin. */
const LIVE: OrderStatus[] = ["PENDING", "IN_PROGRESS", "PAID"];
/** What a card keeps watching: a quote not yet paid can be paid from another card or the chat at any moment, and a
 * card left showing "Confirm payment" for a paid quote is a trap. */
const WATCH: OrderStatus[] = ["NOT_STARTED", ...LIVE];

/** A quote, and once paying starts, its order. Its button is the user's acceptance: it names the total and table,
 * and one tap pays. */
export function QuoteView({
  quoteId,
  result,
  compact,
  track,
}: {
  quoteId: string | null;
  result?: PayLike | null;
  compact: boolean;
  track: boolean;
}) {
  const [quote, setQuote] = useState<Quote | null>(null);
  const [order, setOrder] = useState<Order | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [paid, setPaid] = useState<PayLike | null>(null);
  const [paying, setPaying] = useState(false);
  result = paid ?? result;


  useEffect(() => {
    if (!quoteId) return;
    gql<{ quote: Quote | null }>(Q.QUOTE, { id: quoteId }).then(
      (d) => (d.quote ? setQuote(d.quote) : setError("That quote has gone.")),
      (e) => setError(String(e.message ?? e)),
    );
  }, [quoteId]);

  // Follow the payment while it's moving: the server waits up to 20 s for a change, so this is cheap.
  // Every card watches its quote once loaded, so all the cards for one round agree on whether it's paid.
  const started = track || quote != null || (result?.status && result.status !== "NOT_STARTED");
  useEffect(() => {
    if (!quoteId || !started) return;
    let stop = false;
    (async () => {
      let wait = 0;
      let last: OrderStatus | null = null;
      while (!stop) {
        // An unpaid quote answers at once, so a card checks every few seconds; not while the chat is in the background.
        if (document.hidden) {
          await new Promise((r) => setTimeout(r, 2000));
          continue;
        }
        const asked = Date.now();
        try {
          const { order } = await gql<{ order: Order }>(Q.ORDER, { id: quoteId, wait });
          if (stop) return;
          setOrder(order);
          setQuote(order.quote);
          if (!WATCH.includes(order.status)) return;
          // The server holds the call until something changes; if it answered at once with nothing new, pause.
          if (order.status === last && Date.now() - asked < 2000) await new Promise((r) => setTimeout(r, 5000));
          last = order.status;
          wait = 20;
        } catch {
          await new Promise((r) => setTimeout(r, 5000));
        }
      }
    })();
    return () => {
      stop = true;
    };
  }, [quoteId, started]);

  if (error) return <p class="error">{error}</p>;
  if (!quote) return <Pouring label="Asking the till…" />;

  const status = order?.status ?? result?.status ?? quote.status;
  const page = order?.paymentPage ?? result?.paymentPage ?? null;

  return (
    <article class={`quote ${compact ? "is-compact" : ""}`} aria-labelledby="quote-title" data-quote={quote.id}>
      <header class="quote-head">
        <div>
          <p class="eyebrow">{status === "NOT_STARTED" ? "Quote" : "Order"} · {quote.pubName}</p>
          <h2 id="quote-title" class="quote-total">
            {quote.total.formatted}
          </h2>
          {quote.savings && (
            <p class="small saved">
              <Icon name="star" size={14} /> Deals took off {quote.savings.formatted}
            </p>
          )}
          {quote.differsFromMenu && quote.menuTotal && (
            <p class="small differs">
              Menu said <s>{quote.menuTotal.formatted}</s>; the till’s price is what’s charged.
            </p>
          )}
        </div>
        <div class="table-big static" aria-label={`Table ${quote.table}`}>
          <span>Table</span>
          <b>{quote.table}</b>
        </div>
      </header>

      {/* What's being paid for, always: the button below is the acceptance. A long round shows its first lines. */}
      <ul class="quote-lines">
          {(compact && quote.lines.length > 6 ? quote.lines.slice(0, 5) : quote.lines).map((l, i) => {
            const each = l.menuPrice;
            const till = l.tillPrice;
            const menuLine = each ? each.amount * l.quantity : null;
            const differs = till && menuLine != null && Math.abs(till.amount - menuLine) > 0.005;
            return (
              <li key={i}>
                <span class="q-qty">{l.quantity}×</span>
                <span class="q-name">
                  {l.description}
                  {l.deal && <span class="tag deal">{l.deal}</span>}
                </span>
                <span class="q-price">
                  {differs && each && <s class="muted small">{(menuLine as number).toFixed(2)}</s>} {till?.formatted ?? "—"}
                </span>
              </li>
            );
          })}
        {compact && quote.lines.length > 6 && (
          <li class="muted small">and {quote.lines.length - 5} more</li>
        )}
      </ul>

      <Tracker status={status} order={order} />

      {result?.question && (
        <p class="callout warn">
          <Icon name="chat" size={16} /> {result.question.split(". ")[0]}. Answer in the chat.
        </p>
      )}
      {result?.preview && status === "NOT_STARTED" && !paid && (
        <div class="callout">
          <p class="small">{result.preview}</p>
        </div>
      )}
      {result?.card && <p class="small muted">{result.card}</p>}

      {page && (status === "PENDING" || status === "PREPARING") && (
        <button class="primary wide" onClick={() => void openLink(page)}>
          <Icon name="link" size={16} /> Open the payment page
        </button>
      )}
      {status === "NOT_STARTED" && !paid?.question && (
        <button
          class="primary wide pay"
          disabled={paying}
          onClick={async () => {
            setPaying(true);
            const r = await payNow(quote);
            setPaying(false);
            if (r) setPaid(r);
          }}
        >
          {paying ? "Paying…" : `Confirm payment · ${quote.total.formatted} · table ${quote.table}`}
        </button>
      )}
    </article>
  );
}

/** The server's `meaning` is written for the model; this is for the person holding the phone. */
const FOR_PEOPLE: Partial<Record<OrderStatus, string>> = {
  PREPARING: "Starting the payment…",
  PENDING: "Waiting for your card on the payment page.",
  IN_PROGRESS: "Your bank may ask you to approve this in your banking app.",
  PAID: "Paid. Sending it to the bar…",
  UNCONFIRMED: "The payment didn’t get a clear answer. Ask in the chat before trying again.",
  UNKNOWN: "The payment is in a state this app doesn’t know. Ask in the chat.",
};

const STEPS: { label: string; done: OrderStatus[] }[] = [
  { label: "Quoted", done: ["NOT_STARTED", "PREPARING", "PENDING", "IN_PROGRESS", "PAID", "PLACED", "DUPLICATE"] },
  { label: "Card", done: ["IN_PROGRESS", "PAID", "PLACED", "DUPLICATE"] },
  { label: "Bank OK", done: ["PAID", "PLACED"] },
  { label: "At the bar", done: ["PLACED"] },
];

function Tracker({ status, order }: { status: OrderStatus; order: Order | null }) {
  if (status === "CANCELLED") return <p class="callout">Cancelled. Nothing was charged for this quote.</p>;
  if (status === "DUPLICATE") {
    return (
      <p class="callout warn">
        Held back as a repeat of a recent order to this table. Say in the chat whether to send it anyway or cancel it.
      </p>
    );
  }
  return (
    <div class="tracker" role="status" aria-live="polite">
      <ol>
        {STEPS.map((s, i) => {
          const done = s.done.includes(status);
          const next = !done && (i === 0 || STEPS[i - 1].done.includes(status));
          return (
            <li key={s.label} class={done ? "done" : next && status !== "NOT_STARTED" ? "now" : ""}>
              <span class="step-dot">{done ? <Icon name="check" size={12} /> : null}</span>
              {s.label}
            </li>
          );
        })}
      </ol>
      {order?.orderRef && (
        <p class="order-ref">
          Order <b>#{order.orderRef}</b> is on its way.
        </p>
      )}
      {FOR_PEOPLE[status] && <p class="muted small">{FOR_PEOPLE[status]}</p>}
    </div>
  );
}
