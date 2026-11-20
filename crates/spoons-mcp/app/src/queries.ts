// The app's GraphQL. It goes through the same `spoons` tool the model uses: there is no other way in.

const MONEY = "{ amount formatted }";

export const PRODUCT = `id name shortName brewery kind guest category group tasteNotes origin style abv units kcal
  colour { hex label } orderable whyNot outOfStock vegan
  portions { label name price ${MONEY} isDefault units kcal millilitres }`;

const PUB_CARD = `ref name town postcode distanceMetres trading openNow hoursToday { opens closes closed }
  waitTimes { drinkMinutes foodMinutes }`;

export const PUBS = `query($q: String, $near: LatLngInput) {
  pubs(query: $q, near: $near, first: 8) { totalCount nodes { ${PUB_CARD} } } }`;

export const PUB = `query($ref: Int!) { pub(ref: $ref) { ${PUB_CARD} } }`;

export const MENU = `query($ref: Int!, $table: Int) { pub(ref: $ref) { ${PUB_CARD}
  menu(table: $table) { table totalCount
    sections { kind label count products { ${PRODUCT} } }
    comingSoon { fullName abv style colour { hex label } } } } }`;

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
