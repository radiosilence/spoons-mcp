import { useEffect, useState } from "preact/hooks";
import { gql, tellModel } from "../bridge";
import * as Q from "../queries";
import { go } from "../store";
import type { PubCard, View } from "../types";
import { Icon, Pouring, distance } from "../ui";

type PubsView = Extract<View, { screen: "pubs" }>;

export function Pubs({ view, compact }: { view: PubsView; compact: boolean }) {
  const [pubs, setPubs] = useState<PubCard[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setPubs(null);
    const load = view.pub
      ? gql<{ pub: PubCard | null }>(Q.PUB, { ref: view.pub }).then((d) => (d.pub ? [d.pub] : []))
      : gql<{ pubs: { nodes: PubCard[] } }>(Q.PUBS, { q: view.query ?? null, near: view.near ?? null }).then(
          (d) => d.pubs.nodes,
        );
    load.then(setPubs, (e) => setError(String(e.message ?? e)));
  }, [view.pub, view.query, view.near?.lat, view.near?.lng]);

  if (error) return <p class="error">{error}</p>;
  if (!pubs) return <Pouring label="Finding pubs…" />;
  if (!pubs.length) return <p class="empty">No Wetherspoon’s found{view.query ? ` for “${view.query}”` : ""}.</p>;

  const shown = compact ? pubs.slice(0, 3) : pubs;
  const choose = (pub: PubCard) => {
    void tellModel(`In the Wetherspoon's app, the user opened the menu at ${pub.name} (ref ${pub.ref}).`);
    go({ screen: "menu", pub: pub.ref });
  };

  return (
    <div class={`pubs ${compact ? "is-compact" : ""}`}>
      <h2 class="screen-title">
        {view.near ? "Nearest pubs" : view.query ? `Pubs: “${view.query}”` : "Pubs"}
      </h2>
      <ul class="pub-list">
        {shown.map((pub, i) => (
          <li key={pub.ref} style={{ "--i": i }}>
            <button class="pub-card" onClick={() => choose(pub)} aria-label={`Open the menu at ${pub.name}`}>
              <span class="pub-main">
                <span class="pub-name">{pub.name}</span>
                <span class="pub-where muted">
                  {[pub.town, pub.postcode].filter(Boolean).join(" · ")}
                </span>
                <span class="pub-meta">
                  <OpenState pub={pub} />
                  {pub.waitTimes?.drinkMinutes != null && (
                    <span class="chip-quiet">
                      <Icon name="clock" size={14} /> drinks ~{pub.waitTimes.drinkMinutes} min
                    </span>
                  )}
                </span>
              </span>
              {pub.distanceMetres != null && (
                <span class="pub-distance">
                  <Icon name="pin" size={14} />
                  {distance(pub.distanceMetres)}
                </span>
              )}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function OpenState({ pub }: { pub: PubCard }) {
  if (!pub.trading) return <span class="status closed">Closed for now</span>;
  if (pub.openNow === false) {
    return <span class="status closed">Closed{pub.hoursToday?.opens ? ` · opens ${pub.hoursToday.opens}` : ""}</span>;
  }
  if (pub.openNow) {
    const closes = pub.hoursToday?.closes;
    return <span class="status open">Open{closes ? ` · until ${closes === "00:00" ? "midnight" : closes}` : ""}</span>;
  }
  // Hours unknown: say nothing rather than guess.
  return null;
}
