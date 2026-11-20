// The app's GraphQL. It goes through the same `spoons` tool the model uses: there is no other way in.

const MONEY = "{ amount formatted }";

/** Only what the menu draws: a pub's whole menu is hundreds of products, and every byte crosses the host. */
export const PRODUCT = `id name shortName brewery kind guest tasteNotes origin style abv units
  colour { hex } orderable whyNot vegan portions { name price { formatted } isDefault }`;

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
  lines { id productId name portion quantity kind colour { hex label } unitPrice ${MONEY} lineTotal ${MONEY} note }`;

export const BASKET = `{ basket { ${BASKET_FIELDS} } }`;

export const BASKET_WAIT = `query($v: Int!) { basket(changedFrom: $v, waitSecs: 8) { ${BASKET_FIELDS} } }`;

const QUOTE_FIELDS = `id pubRef pubName table total ${MONEY} menuTotal ${MONEY} differsFromMenu status summary
  lines { productId name portion quantity tillPrice ${MONEY} menuPrice ${MONEY} }`;

export const QUOTE = `query($id: ID!) { quote(id: $id) { ${QUOTE_FIELDS} } }`;

export const ORDER = `query($id: ID!, $wait: Int) { order(quoteId: $id, waitSecs: $wait) {
  status meaning orderRef placedAt paymentPage quote { ${QUOTE_FIELDS} } } }`;

const RESULT = `success error basket { ${BASKET_FIELDS} }`;

export const ADD = `mutation($pub: Int, $product: ID!, $portion: String) {
  basketAdd(pub: $pub, productId: $product, portion: $portion) { ${RESULT} } }`;

export const SET = `mutation($line: ID!, $q: Int!) { basketSet(lineId: $line, quantity: $q) { ${RESULT} } }`;

export const SET_TABLE = `mutation($table: Int!, $pub: Int) { setTable(table: $table, pub: $pub) { ${RESULT} } }`;

export const CLEAR = `mutation { basketClear { ${RESULT} } }`;

export const MAKE_QUOTE = `mutation { quote { success error quote { ${QUOTE_FIELDS} } } }`;
