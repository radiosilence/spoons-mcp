import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { canFullscreen, gql, setDisplayMode, app } from "../bridge";
import * as Q from "../queries";
import { add, go, useStore } from "../store";
import type { Hints, Kind, Menu, Portion, Product, PubCard, Section, View } from "../types";
import { Abv, Glass, Icon, Pouring } from "../ui";

type MenuViewT = Extract<View, { screen: "menu" }>;

type Loaded = PubCard & { menu: Menu };

/** What a search matches: every word, in the name, brewery, style, kind, category or notes. */
function matches(p: Product, words: string[]): boolean {
  const hay = [p.name, p.brewery, p.style, p.category, p.group, p.kind.replace(/_/g, " "), ...p.tasteNotes]
    .filter(Boolean)
    .join(" ")
    .toLowerCase()
    .replace(/[’'`]/g, "");
  return words.every((w) => hay.includes(w));
}

export function MenuView({ view, hints, compact }: { view: MenuViewT; hints: Hints; compact: boolean }) {
  const { basket, busy } = useStore();
  const [data, setData] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState(view.search ?? "");
  const [active, setActive] = useState<Kind | null>(null);
  const [afOnly, setAfOnly] = useState(false);
  const [showFood, setShowFood] = useState(false);
  const [fullscreen, setFullscreen] = useState(app.getHostContext()?.displayMode === "fullscreen");
  const scroller = useRef<HTMLDivElement>(null);

  // The basket's table decides the prices, once there is one.
  const table = basket?.pub?.ref === view.pub && basket.table != null ? basket.table : (view.table ?? null);

  useEffect(() => {
    setError(null);
    gql<{ pub: Loaded | null }>(Q.MENU, { ref: view.pub, table }).then(
      (d) => (d.pub ? setData(d.pub) : setError("No such pub.")),
      (e) => setError(String(e.message ?? e)),
    );
  }, [view.pub, table]);

  useEffect(() => {
    const onMode = () => setFullscreen(app.getHostContext()?.displayMode === "fullscreen");
    app.addEventListener("hostcontextchanged", onMode);
    return () => app.removeEventListener("hostcontextchanged", onMode);
  }, []);

  const words = search.toLowerCase().replace(/[’'`]/g, "").split(/\s+/).filter(Boolean);
  const sections: Section[] = useMemo(() => {
    if (!data) return [];
    return data.menu.sections
      .filter((s) => !afOnly || s.kind === "ALCOHOL_FREE" || s.kind === "SOFT" || s.kind === "HOT")
      .filter((s) => !view.kinds?.length || view.kinds.includes(s.kind))
      .map((s) => ({ ...s, products: words.length ? s.products.filter((p) => matches(p, words)) : s.products }))
      .filter((s) => s.products.length);
  }, [data, words.join(" "), afOnly, view.kinds?.join()]);

  const guests = useMemo(
    () => data?.menu.sections.find((s) => s.kind === "CASK_ALE")?.products.filter((p) => p.guest) ?? [],
    [data],
  );
  const highlight = new Set(hints.highlight ?? []);
  const inBasket = new Map(basket?.lines.map((l) => [l.productId, l.quantity]) ?? []);

  // Bring what the model asked for into view, once.
  useEffect(() => {
    if (!data || !hints.focus) return;
    const el = document.getElementById(`k-${hints.focus}`) ?? document.getElementById(`p-${hints.focus}`);
    el?.scrollIntoView({ block: "start", behavior: "smooth" });
  }, [data, hints.focus]);

  // Which section is on screen, for the tabs.
  useEffect(() => {
    const root = scroller.current;
    if (!root || compact) return;
    const seen = new IntersectionObserver(
      (entries) => {
        const top = entries.filter((e) => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0];
        if (top) setActive(top.target.getAttribute("data-kind") as Kind);
      },
      { root, rootMargin: "-56px 0px -70% 0px" },
    );
    root.querySelectorAll("[data-kind]").forEach((el) => seen.observe(el));
    return () => seen.disconnect();
  }, [sections, compact]);

  if (error) return <p class="error">{error}</p>;
  if (!data) return <Pouring label="Reading the menu…" />;

  const jump = (kind: Kind) => {
    if (kind === "FOOD") setShowFood(true);
    requestAnimationFrame(() => document.getElementById(`k-${kind}`)?.scrollIntoView({ block: "start", behavior: "smooth" }));
  };

  const order = (p: Product, portion: Portion) => void add(view.pub, p, portion);

  if (compact) {
    const shortlist = words.length ? sections.flatMap((s) => s.products).slice(0, 6) : guests.slice(0, 4);
    return (
      <div class="menu is-compact">
        <MenuHeader pub={data} table={table} />
        <Searchbar value={search} onInput={setSearch} />
        <ul class="rows">
          {shortlist.map((p) => (
            <ProductRow key={p.id} p={p} onAdd={order} busy={busy} count={inBasket.get(p.id)} lit={highlight.has(p.id)} />
          ))}
        </ul>
        <button class="link-btn" onClick={() => go({ ...view })}>
          See the whole menu · {data.menu.totalCount} things
        </button>
      </div>
    );
  }

  return (
    <div class={`menu ${fullscreen ? "is-fullscreen" : ""}`}>
      <MenuHeader
        pub={data}
        table={table}
        action={
          canFullscreen() ? (
            <button
              class="icon-btn"
              aria-label={fullscreen ? "Back to the chat" : "Full screen"}
              onClick={() => setDisplayMode(fullscreen ? "inline" : "fullscreen")}
            >
              <Icon name={fullscreen ? "collapse" : "expand"} />
            </button>
          ) : null
        }
      />
      <div class="menu-tools">
        <Searchbar value={search} onInput={setSearch} />
        <button
          class={`toggle ${afOnly ? "on" : ""}`}
          aria-pressed={afOnly}
          aria-label="Alcohol-free and soft drinks only"
          title="Alcohol-free and soft drinks only"
          onClick={() => setAfOnly(!afOnly)}
        >
          <Icon name="leaf" size={16} /> 0%
        </button>
      </div>
      <nav class="tabs" aria-label="Menu sections">
        {sections.map((s) => (
          <button key={s.kind} class={active === s.kind ? "on" : ""} aria-current={active === s.kind} onClick={() => jump(s.kind)}>
            {s.label}
            <span class="count">{s.products.length}</span>
          </button>
        ))}
      </nav>
      <div class="menu-scroll" ref={scroller}>
        {!words.length && !afOnly && guests.length > 0 && (
          <section class="spotlight" aria-labelledby="guest-title">
            <h3 id="guest-title">
              <Icon name="star" size={16} /> Guest ales on now
            </h3>
            <div class="spot-row">
              {guests.map((p, i) => (
                <GuestCard key={p.id} p={p} i={i} onAdd={order} busy={busy} count={inBasket.get(p.id)} lit={highlight.has(p.id)} />
              ))}
            </div>
            {data.menu.comingSoon.length > 0 && (
              <p class="coming muted">
                Coming soon:{" "}
                {data.menu.comingSoon.map((a, i) => (
                  <span key={i} class="soon">
                    <span class="dot" style={{ background: a.colour?.hex ?? "var(--brass)" }} />
                    {a.fullName}
                    {a.abv != null ? ` ${a.abv}%` : ""}
                  </span>
                ))}
              </p>
            )}
          </section>
        )}
        {sections.map((s) => {
          const folded = s.kind === "FOOD" && !showFood && !words.length;
          return (
            <section key={s.kind} id={`k-${s.kind}`} data-kind={s.kind} class={`menu-section kind-${s.kind}`}>
              <h3>
                {s.label}
                {s.kind === "ALCOHOL_FREE" && <span class="af-badge">0.0–0.5%</span>}
              </h3>
              {folded ? (
                <button class="link-btn" onClick={() => setShowFood(true)}>
                  Show {s.products.length} food and snacks
                </button>
              ) : (
                <ul class="rows">
                  {s.products.map((p) => (
                    <ProductRow key={p.id} p={p} onAdd={order} busy={busy} count={inBasket.get(p.id)} lit={highlight.has(p.id)} />
                  ))}
                </ul>
              )}
            </section>
          );
        })}
        {!sections.length && <p class="empty">Nothing matches “{search}”.</p>}
      </div>
      <BasketBar />
    </div>
  );
}

function MenuHeader({ pub, table, action }: { pub: PubCard; table: number | null; action?: preact.ComponentChildren }) {
  return (
    <header class="menu-head">
      <div>
        <h2 class="screen-title">{pub.name}</h2>
        <p class="muted small">
          {pub.openNow === false ? "Closed now" : pub.hoursToday?.closes ? `Open until ${pub.hoursToday.closes === "00:00" ? "midnight" : pub.hoursToday.closes}` : pub.town}
          {pub.waitTimes?.drinkMinutes != null ? ` · drinks ~${pub.waitTimes.drinkMinutes} min` : ""}
        </p>
      </div>
      <button class={`table-chip ${table == null ? "unset" : ""}`} onClick={() => go({ screen: "basket" })}>
        {table == null ? (
          "Set table"
        ) : (
          <>
            <span class="small">Table</span>
            <b>{table}</b>
          </>
        )}
      </button>
      {action}
    </header>
  );
}

function Searchbar({ value, onInput }: { value: string; onInput: (v: string) => void }) {
  return (
    <label class="search">
      <Icon name="search" size={16} />
      <span class="sr-only">Search the menu</span>
      <input
        type="search"
        value={value}
        placeholder="Search the bar"
        onInput={(e) => onInput((e.target as HTMLInputElement).value)}
        enterKeyHint="search"
      />
    </label>
  );
}

function Portions({ p, onAdd, busy }: { p: Product; onAdd: (p: Product, portion: Portion) => void; busy: boolean }) {
  return (
    <span class="portions">
      {p.portions.map((portion) => (
        <button
          key={portion.label}
          class={`portion ${portion.isDefault ? "default" : ""}`}
          disabled={!p.orderable || busy}
          onClick={() => onAdd(p, portion)}
          aria-label={`Add ${portion.name} of ${p.name}, ${portion.price.formatted}`}
        >
          <span class="p-name">{portion.name}</span>
          <span class="p-price">{portion.price.formatted}</span>
        </button>
      ))}
    </span>
  );
}

type RowProps = {
  p: Product;
  onAdd: (p: Product, portion: Portion) => void;
  busy: boolean;
  count?: number;
  lit?: boolean;
};

function ProductRow({ p, onAdd, busy, count, lit }: RowProps) {
  const off = !p.orderable;
  return (
    <li id={`p-${p.id}`} class={`row ${off ? "off" : ""} ${lit ? "lit" : ""}`}>
      <Glass kind={p.kind} colour={p.colour?.hex} size={30} product={p} />
      <div class="row-body">
        <div class="row-title">
          <span class="row-name">
            {p.guest && p.brewery ? (
              <>
                <span class="brewery">{p.brewery}</span> {p.shortName}
              </>
            ) : (
              p.name
            )}
          </span>
          {count ? <span class="in-basket" aria-label={`${count} in the basket`}>×{count}</span> : null}
        </div>
        <div class="row-facts">
          <Abv abv={p.abv} units={p.units} />
          {p.origin && <span class="muted small">{p.origin}</span>}
          {p.vegan && <span class="tag">vegan</span>}
        </div>
        {p.tasteNotes.length > 0 && <div class="notes">{p.tasteNotes.join(" · ")}</div>}
        {off ? <div class="why-not">{p.whyNot}</div> : <Portions p={p} onAdd={onAdd} busy={busy} />}
      </div>
    </li>
  );
}

function GuestCard({ p, i, ...rest }: RowProps & { i: number }) {
  return (
    <article id={`p-${p.id}`} class={`guest ${rest.lit ? "lit" : ""}`} style={{ "--i": i, "--beer": p.colour?.hex ?? "#b8732e" }}>
      <div class="guest-top">
        <Glass kind={p.kind} colour={p.colour?.hex} size={44} />
        <div>
          <div class="brewery">{p.brewery}</div>
          <div class="guest-name">{p.shortName}</div>
          <div class="muted small">{p.style ?? p.colour?.label}</div>
        </div>
        {rest.count ? <span class="in-basket">×{rest.count}</span> : null}
      </div>
      <Abv abv={p.abv} units={p.units} />
      {p.tasteNotes.length > 0 && (
        <div class="note-chips">
          {p.tasteNotes.map((n) => (
            <span key={n}>{n}</span>
          ))}
        </div>
      )}
      <Portions p={p} onAdd={rest.onAdd} busy={rest.busy} />
    </article>
  );
}

export function BasketBar() {
  const { basket } = useStore();
  if (!basket || !basket.count) return null;
  return (
    <button class="basket-bar" onClick={() => go({ screen: "basket" })}>
      <span class="bb-count">{basket.count}</span>
      <span class="bb-label">
        {basket.table != null ? `Table ${basket.table}` : "No table yet"}
        <span class="muted small"> · review</span>
      </span>
      <span class="bb-total">{basket.menuTotal?.formatted ?? ""}</span>
    </button>
  );
}
