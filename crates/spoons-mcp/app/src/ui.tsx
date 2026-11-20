// Small pieces every screen uses: the glass a drink is drawn in, strength bars, steppers.

import type { ComponentChildren } from "preact";
import type { Kind, Product } from "./types";

type Shape = "pint" | "wine" | "tumbler" | "mug" | "plate";

function shapeOf(kind: Kind, product?: Pick<Product, "name" | "portions">): Shape {
  switch (kind) {
    case "CASK_ALE":
    case "LAGER":
    case "CRAFT_AND_WORLD":
    case "STOUT":
    case "CIDER":
      return "pint";
    case "WINE":
      return "wine";
    case "HOT":
      return "mug";
    case "FOOD":
    case "OTHER":
      return "plate";
    case "ALCOHOL_FREE":
      return product?.portions.some((p) => /pint|bottle|can/i.test(p.name)) ? "pint" : "tumbler";
    default:
      return "tumbler";
  }
}

const FALLBACK: Partial<Record<Kind, string>> = {
  SOFT: "#c9772b",
  SPIRITS: "#e7c992",
  COCKTAILS: "#e06a5a",
  ALCOHOL_FREE: "#e9c46a",
  HOT: "#5b3a24",
};

let gradientId = 0;

/** The drink, in its glass, in its colour. Decorative: the name beside it says what it is. */
export function Glass({
  kind,
  colour,
  size = 40,
  product,
}: {
  kind: Kind;
  colour?: string | null;
  size?: number;
  product?: Pick<Product, "name" | "portions">;
}) {
  const shape = shapeOf(kind, product);
  const fill = colour ?? FALLBACK[kind] ?? "#d9a227";
  const id = `g${++gradientId}`;
  const dark = luminance(fill) < 0.12;
  const foam = kind === "STOUT" || dark ? "#efe2c4" : "#fbf6ea";
  const w = size;
  const h = Math.round(size * 1.4);
  if (shape === "plate") {
    return (
      <svg width={w} height={h} viewBox="0 0 40 56" aria-hidden="true" class="glass">
        <ellipse cx="20" cy="34" rx="17" ry="7" fill="var(--glass-rim)" />
        <ellipse cx="20" cy="33" rx="12" ry="4.5" fill="var(--bg)" stroke="var(--glass-edge)" />
        <path d="M13 31c2-5 12-5 14 0" fill="#c98d3c" opacity=".85" />
      </svg>
    );
  }
  const outline = {
    pint: "M7 6h26l-2.2 44a3 3 0 0 1-3 2.8H12.2a3 3 0 0 1-3-2.8z",
    wine: "M9 6h22c0 11-4 18-11 19-7-1-11-8-11-19zM19 25h2v20h6v3H13v-3h6z",
    tumbler: "M8 18h24l-2 31a3 3 0 0 1-3 2.6H13a3 3 0 0 1-3-2.6z",
    mug: "M8 18h20v28a4 4 0 0 1-4 4H12a4 4 0 0 1-4-4zM28 23h3a5 5 0 0 1 0 12h-3v-3h3a2 2 0 0 0 0-6h-3z",
  }[shape];
  const liquid = {
    pint: "M8.6 14h22.8l-1.8 36a2 2 0 0 1-2 1.8H12.4a2 2 0 0 1-2-1.8z",
    wine: "M10.2 12h19.6c-.6 7-4 11.6-9.8 12.3-5.8-.7-9.2-5.3-9.8-12.3z",
    tumbler: "M9.4 26h21.2l-1.5 22.7a2 2 0 0 1-2 1.8H13a2 2 0 0 1-2-1.8z",
    mug: "M9.5 24h17v21.5a3 3 0 0 1-3 3h-11a3 3 0 0 1-3-3z",
  }[shape];
  return (
    <svg width={w} height={h} viewBox="0 0 40 56" aria-hidden="true" class="glass">
      <defs>
        <linearGradient id={id} x1="0" x2="1">
          <stop offset="0" stop-color={fill} stop-opacity=".95" />
          <stop offset=".45" stop-color={lighten(fill, 0.18)} />
          <stop offset="1" stop-color={fill} />
        </linearGradient>
      </defs>
      <path d={liquid} fill={`url(#${id})`} />
      {shape === "pint" && <rect x="8.4" y="9" width="23.2" height="6" rx="2.6" fill={foam} />}
      {shape === "mug" && <ellipse cx="18" cy="24.5" rx="8.5" ry="1.6" fill={lighten(fill, 0.35)} />}
      <path d={outline} fill="none" stroke="var(--glass-edge)" stroke-width="1.2" stroke-linejoin="round" />
      <path d="M11 16v28" stroke="#fff" stroke-opacity=".35" stroke-width="1.6" stroke-linecap="round" />
    </svg>
  );
}

function hexToRgb(hex: string): [number, number, number] {
  const m = hex.replace("#", "");
  const n = parseInt(m.length === 3 ? m.replace(/./g, "$&$&") : m.padEnd(6, "0"), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function luminance(hex: string): number {
  const [r, g, b] = hexToRgb(hex).map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function lighten(hex: string, by: number): string {
  const [r, g, b] = hexToRgb(hex).map((c) => Math.round(c + (255 - c) * by));
  return `rgb(${r},${g},${b})`;
}

/** Strength on a scale to 12%, so a 3.4% bitter and a 5% lager look as different as they are. */
export function Abv({ abv, units }: { abv: number | null; units?: number | null }) {
  if (abv == null) return null;
  const pct = Math.min(abv / 12, 1) * 100;
  return (
    <span class="abv" title={`${abv}% ABV`}>
      <span class="abv-bar" aria-hidden="true">
        <span style={{ width: `${pct}%` }} />
      </span>
      <span class="abv-num">
        {abv === 0 ? "0.0" : abv}%{units != null && units > 0 ? <span class="muted"> · {units} u</span> : null}
      </span>
    </span>
  );
}

export function Stepper({
  value,
  onChange,
  label,
  busy,
}: {
  value: number;
  onChange: (next: number) => void;
  label: string;
  busy?: boolean;
}) {
  return (
    <span class="stepper" role="group" aria-label={`Quantity of ${label}`}>
      <button type="button" aria-label={`One fewer ${label}`} disabled={busy} onClick={() => onChange(value - 1)}>
        {value === 1 ? <Icon name="bin" /> : "−"}
      </button>
      <output aria-live="polite">{value}</output>
      <button type="button" aria-label={`One more ${label}`} disabled={busy || value >= 10} onClick={() => onChange(value + 1)}>
        +
      </button>
    </span>
  );
}

const ICONS = {
  bin: "M5 7h14M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3",
  search: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4",
  expand: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
  collapse: "M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5",
  pin: "M12 21s-6-5.3-6-11a6 6 0 0 1 12 0c0 5.7-6 11-6 11zM12 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z",
  back: "M15 5l-7 7 7 7",
  chat: "M4 5h16v11H9l-5 4z",
  link: "M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5",
  clock: "M12 21a9 9 0 1 1 0-18 9 9 0 0 1 0 18zM12 7v5l3 2",
  check: "M5 12l5 5 9-10",
  leaf: "M5 19c0-8 6-14 14-14 0 8-6 14-14 14zM5 19l7-7",
  star: "M12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1L3.2 9.5l6.1-.9z",
};

export function Icon({ name, size = 18 }: { name: keyof typeof ICONS; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
      class="icon"
    >
      <path d={ICONS[name]} />
    </svg>
  );
}

export function distance(metres: number | null): string | null {
  if (metres == null) return null;
  return metres < 1000 ? `${Math.round(metres / 10) * 10} m` : `${(metres / 1000).toFixed(1)} km`;
}

export function Card({ children, class: cls = "" }: { children: ComponentChildren; class?: string }) {
  return <section class={`card ${cls}`}>{children}</section>;
}

export function Pouring({ label = "Pouring…" }: { label?: string }) {
  return (
    <div class="pouring" role="status">
      <span class="pour-glass" aria-hidden="true">
        <span />
      </span>
      {label}
    </div>
  );
}
