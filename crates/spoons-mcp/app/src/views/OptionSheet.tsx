import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { gql } from "../bridge";
import * as Q from "../queries";
import { add } from "../store";
import type { OptionGroup, Portion, Product } from "../types";
import { Glass, Icon, Pouring } from "../ui";

type Drink = Pick<Product, "id" | "name" | "kind" | "colour" | "needsChoices" | "portions">;

/** What has been tapped: an option id per group, and the meal's drink. */
export type Answers = { picked: Record<string, string>; drink: Drink | null };

/** A product's questions, answered by tapping: a mixer, a side, peas; and for a meal deal, the drink. */
export function OptionSheet(props: {
  pub: number;
  table: number | null;
  product: Product;
  portion: Portion;
  answers: Answers;
  onAnswer: (answers: Answers) => void;
  onClose: () => void;
}) {
  const { product, portion } = props;
  const [groups, setGroups] = useState<OptionGroup[] | null>(null);
  const [drinks, setDrinks] = useState<Drink[]>([]);
  const { picked, drink } = props.answers;
  const setPicked = (next: Record<string, string>) => props.onAnswer({ picked: next, drink });
  const setDrink = (next: Drink | null) => props.onAnswer({ picked, drink: next });
  const [find, setFind] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const sheet = useRef<HTMLDivElement>(null);

  useEffect(() => {
    gql<{ pub: { menu: { product: { optionGroups: OptionGroup[]; includedDrinks: Drink[] } } } }>(Q.OPTIONS, {
      ref: props.pub,
      table: props.table,
      id: product.id,
    }).then(
      (d) => {
        const p = d.pub.menu.product;
        setGroups(p.optionGroups.filter((g) => !g.sizes.length || g.sizes.includes(portion.name)));
        // A drink that asks its own question (a spirit's mixer) is for the chat; the sheet offers the rest.
        setDrinks(p.includedDrinks.filter((d) => !d.needsChoices));
      },
      (e) => setError(String(e.message ?? e)),
    );
  }, [product.id, portion.name]);

  useEffect(() => {
    sheet.current?.focus();
    const esc = (e: KeyboardEvent) => e.key === "Escape" && props.onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, []);

  const missing = groups?.filter((g) => g.required && !picked[g.id]) ?? [];
  const shownDrinks = useMemo(() => {
    const words = find.toLowerCase().split(/\s+/).filter(Boolean);
    const matching = drinks.filter((d) => words.every((w) => d.name.toLowerCase().includes(w)));
    return matching.slice(0, words.length ? 24 : 12);
  }, [drinks, find]);

  const confirm = async () => {
    if (!groups || missing.length) return;
    setBusy(true);
    const options = Object.entries(picked).map(([group, option]) => ({ group, options: [option] }));
    const said = groups.flatMap((g) => g.options.filter((o) => o.id === picked[g.id]).map((o) => o.label));
    const ok = await add(props.pub, props.table, product, portion, {
      options,
      said,
      drink: drink ? { productId: drink.id, name: drink.name } : undefined,
    });
    setBusy(false);
    if (ok) props.onClose();
  };

  return (
    <div class="sheet-backdrop" onClick={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="sheet-title" tabIndex={-1} ref={sheet}>
        <header class="sheet-head">
          <div>
            <h3 id="sheet-title">{product.name}</h3>
            <p class="muted small">
              {portion.name} · {portion.price.formatted}
            </p>
          </div>
          <button class="icon-btn" aria-label="Close" onClick={props.onClose}>
            ✕
          </button>
        </header>
        {error && <p class="error">{error}</p>}
        {!groups && !error && <Pouring label="Asking the kitchen…" />}
        {groups?.map((g) => (
          <fieldset key={g.id} class="choice-group">
            <legend>
              {g.name}
              {g.required && <span class={picked[g.id] ? "tag done" : "tag"}>{picked[g.id] ? "✓" : "Pick one"}</span>}
            </legend>
            <div class="choice-chips">
              {g.options.map((o) => (
                <button
                  key={o.id}
                  type="button"
                  class={`choice ${picked[g.id] === o.id ? "on" : ""}`}
                  aria-pressed={picked[g.id] === o.id}
                  disabled={!o.available}
                  onClick={() => setPicked({ ...picked, [g.id]: o.id })}
                >
                  {o.label}
                  {o.price.amount > 0 && <span class="muted"> +{o.price.formatted}</span>}
                </button>
              ))}
            </div>
          </fieldset>
        ))}
        {groups && drinks.length > 0 && (
          <fieldset class="choice-group">
            <legend>
              A drink with your meal <span class="tag deal">Meal deal</span>
            </legend>
            <label class="search small-search">
              <Icon name="search" size={14} />
              <span class="sr-only">Find a drink</span>
              <input type="search" placeholder="Find a drink" value={find} onInput={(e) => setFind((e.target as HTMLInputElement).value)} />
            </label>
            <div class="choice-chips">
              <button type="button" class={`choice ${drink === null ? "on" : ""}`} aria-pressed={drink === null} onClick={() => setDrink(null)}>
                No drink
              </button>
              {shownDrinks.map((d) => (
                <button
                  key={d.id}
                  type="button"
                  class={`choice drink ${drink?.id === d.id ? "on" : ""}`}
                  aria-pressed={drink?.id === d.id}
                  onClick={() => setDrink(d)}
                >
                  <Glass kind={d.kind} colour={d.colour?.hex} size={14} product={d} />
                  {d.name}
                </button>
              ))}
            </div>
          </fieldset>
        )}
        {groups && (
          <footer class="sheet-foot">
            <button class="primary wide" disabled={busy || missing.length > 0} onClick={() => void confirm()}>
              {missing.length ? `Choose ${missing[0].name.replace(/\?$/, "").toLowerCase()}` : drink ? `Add with ${drink.name}` : "Add to the round"}
            </button>
          </footer>
        )}
      </div>
    </div>
  );
}
