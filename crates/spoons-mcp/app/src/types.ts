// The shapes the app's own queries select. Field names match the GraphQL schema (crates/spoons-mcp/schema.graphql).

export type Kind =
  | "CASK_ALE"
  | "LAGER"
  | "CRAFT_AND_WORLD"
  | "STOUT"
  | "CIDER"
  | "WINE"
  | "SPIRITS"
  | "COCKTAILS"
  | "ALCOHOL_FREE"
  | "SOFT"
  | "HOT"
  | "FOOD"
  | "OTHER";

export interface Money {
  amount: number;
  formatted: string;
}

export interface Colour {
  hex: string;
  label: string;
}

export interface Portion {
  name: string;
  price: { formatted: string };
  isDefault: boolean;
}

export interface Product {
  id: string;
  name: string;
  shortName: string;
  brewery: string | null;
  kind: Kind;
  guest: boolean;
  tasteNotes: string[];
  origin: string | null;
  style: string | null;
  abv: number | null;
  units: number | null;
  colour: { hex: string } | null;
  portions: Portion[];
  orderable: boolean;
  whyNot: string | null;
  vegan: boolean;
  /** Asks a question (a mixer, a side) before it can be ordered. */
  needsChoices: boolean;
  deals: { kind: "INCLUDES_DRINK" | "MULTI_BUY" | "OTHER" }[];
}

export interface OptionGroup {
  id: string;
  name: string;
  required: boolean;
  sizes: string[];
  options: { id: string; label: string; price: Money; available: boolean }[];
}

/** One question's answer, as `basketAdd` takes it. */
export interface OptionPick {
  group: string;
  options: string[];
}

export interface Section {
  kind: Kind;
  label: string;
  count: number;
  products: Product[];
}

export interface Hours {
  opens: string | null;
  closes: string | null;
  closed: boolean;
}

export interface PubCard {
  ref: number;
  name: string;
  town: string | null;
  postcode: string | null;
  distanceMetres: number | null;
  trading: boolean;
  openNow: boolean | null;
  hoursToday: Hours | null;
  waitTimes: { drinkMinutes: number | null; foodMinutes: number | null } | null;
}

export interface Menu {
  table: number | null;
  totalCount: number;
  /** Every kind on the menu with its count, loaded or not. */
  all: { kind: Kind; label: string; count: number }[];
  sections: Section[];
  comingSoon: { fullName: string | null; abv: number | null; colour: { hex: string } | null }[];
}

export interface BasketLine {
  id: string;
  productId: string;
  name: string;
  portion: string;
  quantity: number;
  options: string[];
  includedWith: string | null;
  kind: Kind;
  colour: Colour | null;
  unitPrice: Money | null;
  lineTotal: Money | null;
  note: string | null;
}

export interface Basket {
  version: number;
  table: number | null;
  count: number;
  ready: boolean;
  summary: string;
  menuTotal: Money | null;
  pub: { ref: number; name: string } | null;
  lines: BasketLine[];
}

export type OrderStatus =
  | "NOT_STARTED"
  | "PREPARING"
  | "UNCONFIRMED"
  | "PENDING"
  | "IN_PROGRESS"
  | "PAID"
  | "PLACED"
  | "DUPLICATE"
  | "CANCELLED"
  | "UNKNOWN";

export interface QuoteLine {
  productId: string;
  name: string;
  portion: string;
  quantity: number;
  description: string;
  deal: string | null;
  tillPrice: Money | null;
  menuPrice: Money | null;
}

export interface Quote {
  id: string;
  pubRef: number;
  pubName: string;
  table: number;
  total: Money;
  menuTotal: Money | null;
  savings: Money | null;
  differsFromMenu: boolean;
  status: OrderStatus;
  summary: string;
  lines: QuoteLine[];
}

export interface Order {
  status: OrderStatus;
  meaning: string;
  orderRef: string | null;
  placedAt: string | null;
  paymentPage: string | null;
  quote: Quote;
}

/** What the server says fits a result: see `crates/spoons-mcp/src/ui.rs`. */
export type View =
  | { screen: "pubs"; query?: string | null; near?: { lat: number; lng: number } | null; pub?: number | null }
  | { screen: "menu"; pub: number; table?: number | null; search?: string | null; kinds?: Kind[] | null }
  | { screen: "basket"; result?: unknown }
  | { screen: "quote"; quoteId: string | null; action?: string; result?: PayLike | null }
  | { screen: "order"; quoteId: string };

/** The parts of a `pay`, `cancelOrder` or `sendDuplicate` result the app shows. */
export interface PayLike {
  success?: boolean;
  preview?: string | null;
  paymentPage?: string | null;
  question?: string | null;
  status?: OrderStatus | null;
  card?: string | null;
  next?: string | null;
  error?: string | null;
}

export interface Hints {
  view?: "COMPACT" | "FULL" | "NONE" | null;
  focus?: string | null;
  highlight?: string[] | null;
}
