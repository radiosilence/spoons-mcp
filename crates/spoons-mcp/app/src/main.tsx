import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { app, applyHost, setSession } from "./bridge";
import { back, set, useStore, watchBasket } from "./store";
import "./styles.css";
import type { Hints, View } from "./types";
import { Icon, Pouring } from "./ui";
import { BasketView } from "./views/BasketView";
import { MenuView } from "./views/MenuView";
import { Pubs } from "./views/Pubs";
import { QuoteView } from "./views/QuoteView";

interface Structured {
  view?: View | null;
  ui?: Hints | null;
  session?: string;
}

/** Results about money or a basket are small; lists and menus deserve the room. */
function compactByDefault(view: View | null, hints: Hints): boolean {
  if (hints.view === "COMPACT") return true;
  if (hints.view === "FULL") return false;
  return view?.screen === "basket";
}

// Handlers go on before connecting, so the first result isn't missed.
let waiting = true;
app.ontoolinput = () => {
  waiting = true;
};
app.ontoolresult = (result) => {
  waiting = false;
  const s = (result.structuredContent ?? {}) as Structured;
  setSession(s.session);
  const hints = s.ui ?? {};
  const view = s.view ?? null;
  set({ view, hints, compact: compactByDefault(view, hints), history: [] });
};
app.ontoolcancelled = () => {
  waiting = false;
  set({ view: null });
};
app.onhostcontextchanged = (ctx) => applyHost({ ...app.getHostContext(), ...ctx });

function Root() {
  const { view, hints, compact, toast, history } = useStore();
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    const stop = new AbortController();
    app.connect().then(() => {
      applyHost(app.getHostContext());
      setConnected(true);
      void watchBasket(stop.signal);
    }, console.error);
    app.onteardown = async () => {
      stop.abort();
      return {};
    };
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    document.documentElement.classList.toggle("still", reduced.matches);
    return () => stop.abort();
  }, []);

  if (!view) {
    // Nothing to show for this call (the time, the schema), or still waiting for the result.
    return connected && waiting ? <Pouring /> : null;
  }

  const screen = (() => {
    switch (view.screen) {
      case "pubs":
        return <Pubs view={view} compact={compact} />;
      case "menu":
        return <MenuView view={view} hints={hints} compact={compact} />;
      case "basket":
        return <BasketView compact={compact} />;
      case "quote":
        return <QuoteView quoteId={view.quoteId} result={view.result} compact={compact} track={false} />;
      case "order":
        return <QuoteView quoteId={view.quoteId} compact={compact} track />;
    }
  })();

  return (
    <main class={`shell screen-${view.screen} ${compact ? "compact" : ""}`}>
      {(history.length > 0 || compact) && (
        <div class="shell-nav">
          {history.length > 0 ? (
            <button class="icon-btn" aria-label="Back" onClick={back}>
              <Icon name="back" />
            </button>
          ) : (
            <span />
          )}
          {compact && view.screen !== "basket" && (
            <button class="link-btn" onClick={() => set({ compact: false })}>
              Show more
            </button>
          )}
        </div>
      )}
      <div class="screen" key={view.screen}>
        {screen}
      </div>
      {toast && (
        <div class={`toast ${toast.tone}`} role="status">
          {toast.text}
        </div>
      )}
    </main>
  );
}

render(<Root />, document.getElementById("root")!);
