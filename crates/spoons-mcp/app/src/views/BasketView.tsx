import { useState } from "preact/hooks";
import { clear, go, quote, setQuantity, setTable, useStore } from "../store";
import { Glass, Pouring, Stepper } from "../ui";

export function BasketView({ compact }: { compact: boolean }) {
  const { basket, busy } = useStore();
  const [table, setTableDraft] = useState("");
  if (!basket) return <Pouring label="Fetching the basket…" />;

  const saveTable = (e: Event) => {
    e.preventDefault();
    const n = parseInt(table, 10);
    if (n > 0) void setTable(n, basket.pub?.ref ?? null).then(() => setTableDraft(""));
  };

  if (compact) {
    return (
      <button class="basket-compact" onClick={() => go({ screen: "basket" })} aria-label="Open the basket">
        <span class="bb-count">{basket.count}</span>
        <span class="bc-text">
          <b>{basket.count ? basket.lines.map((l) => `${l.quantity}× ${l.name}`).join(", ") : "Empty basket"}</b>
          <span class="muted small">
            {basket.pub?.name ?? ""}
            {basket.table != null ? ` · table ${basket.table}` : basket.pub ? " · no table yet" : ""}
          </span>
        </span>
        <span class="bb-total">{basket.menuTotal?.formatted ?? ""}</span>
      </button>
    );
  }

  return (
    <div class="basket">
      <header class="basket-head">
        <div>
          <h2 class="screen-title">Your round</h2>
          <p class="muted small">{basket.pub ? basket.pub.name : "Pick a pub to start"}</p>
        </div>
        <form class={`table-big ${basket.table == null ? "unset" : ""}`} onSubmit={saveTable}>
          <label for="table-input">Table</label>
          <input
            id="table-input"
            inputMode="numeric"
            pattern="[0-9]*"
            placeholder={basket.table != null ? String(basket.table) : "?"}
            value={table}
            onInput={(e) => setTableDraft((e.target as HTMLInputElement).value.replace(/\D/g, ""))}
            aria-describedby="table-help"
          />
          {table && (
            <button type="submit" disabled={busy}>
              Set
            </button>
          )}
        </form>
      </header>
      {basket.table == null && basket.pub && (
        <p id="table-help" class="hint">
          The number is on your table. Drinks go to whichever table is set, so it’s never guessed.
        </p>
      )}
      {basket.lines.length === 0 ? (
        <p class="empty">
          Nothing yet.{" "}
          {basket.pub && (
            <button class="link-btn" onClick={() => go({ screen: "menu", pub: basket.pub!.ref })}>
              Open the menu
            </button>
          )}
        </p>
      ) : (
        <ul class="lines">
          {basket.lines.map((l) => (
            <li key={l.id} class={l.note ? "line off" : "line"}>
              <Glass kind={l.kind} colour={l.colour?.hex} size={26} />
              <div class="line-body">
                <div class="line-name">{l.name}</div>
                <div class="muted small">
                  {[l.portion.replace(/\s*\(.*\)/, ""), ...l.options].filter((x) => x && x !== "Standard").join(", ")}
                  {l.unitPrice ? ` · ${l.unitPrice.formatted} each` : ""}
                </div>
                {l.includedWith && <div class="tag deal">With the meal deal</div>}
                {l.note && <div class="why-not">{l.note}</div>}
              </div>
              {l.includedWith ? (
                <span class="line-qty">×{l.quantity}</span>
              ) : (
                <Stepper value={l.quantity} label={l.name} busy={busy} onChange={(q) => void setQuantity(l.id, l.name, q)} />
              )}
              <span class="line-total">{l.lineTotal?.formatted ?? "—"}</span>
            </li>
          ))}
        </ul>
      )}
      {basket.lines.length > 0 && (
        <footer class="basket-foot">
          <div class="total-row">
            <span>On the menu</span>
            <b class="total">{basket.menuTotal?.formatted ?? "—"}</b>
          </div>
          <p class="muted small">The till’s price, which is what’s charged, comes with the quote.</p>
          <div class="actions">
            <button class="ghost" disabled={busy} onClick={() => void clear()}>
              Empty
            </button>
            {basket.pub && (
              <button class="ghost" onClick={() => go({ screen: "menu", pub: basket.pub!.ref })}>
                Add more
              </button>
            )}
            <button class="primary" disabled={busy || !basket.ready} onClick={() => void quote()}>
              {basket.table == null ? "Set a table first" : "Get the till price"}
            </button>
          </div>
        </footer>
      )}
    </div>
  );
}
