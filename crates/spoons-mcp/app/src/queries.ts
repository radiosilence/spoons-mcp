// The app's GraphQL. It goes through the same `spoons` tool the model uses: there is no other way in.

const MONEY = "{ amount formatted }";

/** Only what the menu draws: a pub's whole menu is hundreds of products, and every byte crosses the host. */
export const PRODUCT = `id name shortName brewery kind guest tasteNotes origin style abv units
  colour { hex } orderable whyNot vegan needsChoices deals { kind }
  portions { name price { formatted } isDefault }`;

/** What a product asks, and the drinks a meal comes with, fetched when someone taps it. */
export const OPTIONS = `query($ref: Int!, $table: Int, $id: ID!) { pub(ref: $ref) { menu(table: $table) {
  product(id: $id) {
    optionGroups { id name required sizes options { id label price { amount formatted } available } }
    includedDrinks(first: 60) { id name kind colour { hex } needsChoices portions { name price { formatted } isDefault } }
  } } } }`;

const PUB_CARD = `ref name town postcode distanceMetres trading openNow hoursToday { opens closes closed }
  waitTimes { drinkMinutes foodMinutes }`;

export const PUBS = `query($q: String, $near: LatLngInput) {
  pubs(query: $q, near: $near, first: 8) { totalCount nodes { ${PUB_CARD} } } }`;

export const PUB = `query($ref: Int!) { pub(ref: $ref) { ${PUB_CARD} } }`;

/** Drinks first; food, half of a menu, when someone opens it. */
export const DRINKS: string[] = ["CASK_ALE", "LAGER", "CRAFT_AND_WORLD", "STOUT", "CIDER", "WINE", "SPIRITS",
  "COCKTAILS", "ALCOHOL_FREE", "SOFT", "HOT"];

export const MENU = `query($ref: Int!, $table: Int, $kinds: [Kind!]) { pub(ref: $ref) { ${PUB_CARD}
  menu(table: $table) { table totalCount
    all: sections { kind label count }
    sections(kinds: $kinds) { kind label count products { ${PRODUCT} } }
    comingSoon { fullName abv colour { hex } } } } }`;

export const MENU_KINDS = `query($ref: Int!, $table: Int, $kinds: [Kind!]) { pub(ref: $ref) {
  menu(table: $table) { sections(kinds: $kinds) { kind label count products { ${PRODUCT} } } } } }`;

export const BASKET_FIELDS = `version table count ready summary menuTotal ${MONEY} pub { ref name }
  lines { id productId name portion quantity options includedWith kind colour { hex label } unitPrice ${MONEY} lineTotal ${MONEY} note }`;

export const BASKET = `{ basket { ${BASKET_FIELDS} } }`;

export const BASKET_WAIT = `query($v: Int!) { basket(changedFrom: $v, waitSecs: 8) { ${BASKET_FIELDS} } }`;

const QUOTE_FIELDS = `id pubRef pubName table total ${MONEY} menuTotal ${MONEY} savings ${MONEY} differsFromMenu
  status summary lines { productId name portion quantity description deal tillPrice ${MONEY} menuPrice ${MONEY} }`;

export const QUOTE = `query($id: ID!) { quote(id: $id) { ${QUOTE_FIELDS} } }`;

const PAY_FIELDS = "success error preview confirmationToken status paymentPage card question next";

/** What paying will do, and the token that does it. Changes nothing. */
export const PAY_PREVIEW = `mutation($id: ID!) { pay(quoteId: $id, action: PREVIEW) { ${PAY_FIELDS} } }`;

/** Pays: on the user's tap of the Pay button, never on its own. */
export const PAY_CONFIRM = `mutation($id: ID!, $token: String!) {
  pay(quoteId: $id, action: CONFIRM, confirmationToken: $token) { ${PAY_FIELDS} } }`;

export const ORDER = `query($id: ID!, $wait: Int) { order(quoteId: $id, waitSecs: $wait) {
  status meaning orderRef placedAt paymentPage quote { ${QUOTE_FIELDS} } } }`;

const RESULT = `success error basket { ${BASKET_FIELDS} }`;

export const ADD = `mutation($pub: Int, $table: Int, $product: ID!, $portion: String, $options: [OptionPickInput!],
  $drink: IncludedDrinkInput) {
  basketAdd(pub: $pub, table: $table, productId: $product, portion: $portion, options: $options, includeDrink: $drink) {
    ${RESULT} } }`;

export const SET = `mutation($line: ID!, $q: Int!) { basketSet(lineId: $line, quantity: $q) { ${RESULT} } }`;

export const SET_TABLE = `mutation($table: Int!, $pub: Int) { setTable(table: $table, pub: $pub) { ${RESULT} } }`;

export const CLEAR = `mutation { basketClear { ${RESULT} } }`;

export const MAKE_QUOTE = `mutation { quote { success error quote { ${QUOTE_FIELDS} } } }`;

/** The server's forgiving, ranked search, for what the instant filter can't place. */
export const SEARCH = `query($ref: Int!, $table: Int, $s: String) { pub(ref: $ref) {
  menu(table: $table) { products(search: $s, first: 12) { closeMatches nodes { ${PRODUCT} } } } } }`;
