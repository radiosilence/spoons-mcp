# spoons-mcp

Wetherspoon's pubs, menus, prices, real ales and wait times, from the API behind its order-and-pay app.

The API is unofficial and undocumented. It is REST, behind a proxy at `ca.jdw-apps.net`, and every route except the
venue list sits under the franchise (`/jdw/…`). The bearer token is built into the order-and-pay site's public
JavaScript bundle, so the client reads it from there at start-up instead of anyone keeping a copy. Orders and
payments go through a second API, `api.jdwpayments.net`, which takes no token.

## Install

Each [GitHub release](https://github.com/radiosilence/spoons-mcp/releases) carries the `spoons` binary for Linux
(static, x86_64 and arm64) and Apple silicon macOS. From a checkout, `cargo install --path crates/spoons-mcp`. The
image `ghcr.io/radiosilence/spoons-mcp` runs the MCP server over HTTP.

## Dump

```console
$ spoons dump                     # drinks and real ale menus for every pub, into ./data
$ spoons dump --menus all         # every menu, food included
$ spoons dump --only 254,2963     # just these pubs, by venue ref
$ spoons prices                   # rebuild data/prices.jsonl from what's been dumped
$ spoons tiers                    # rank pubs by what a round costs, into data/tiers.md and tiers.json
```

Each body is saved exactly as sent, under `data/pubs/<venueRef>/`, so fields the client doesn't model are still
there. Menus are gzipped: one pub's drinks menu is about 5 MB of JSON, most of it the same options repeated, and
about 330 KB compressed. The dump is resumable and skips anything already saved, and requests are paced at two a
second by default (`--interval-ms`), since a full run is several thousand of them.

`prices.jsonl` has one line per drink, size and pub. A drink listed in several categories appears once per
category, because a meal deal's "includes a drink" price is not the drink's own price.

## Tiers

A round is a pint each of Carling, Guinness, Abbot Ale and Strongbow, a 250 ml glass of Coldwater Creek Merlot and a
double Bell's with a mixer. These are sold at nearly every pub and their prices vary; Abbot stands in for Doom Bar,
which one pub in seven doesn't sell. Each pub's price for each drink is set against the median across all pubs, and
the average of those ratios is the pub's index, so a pub missing one drink still ranks fairly. Tiers are by rank, S
being the cheapest tenth and F the dearest twentieth; pubs charging the same share a tier, since prices come in zones. Airport pubs are ranked separately, as an airside price says nothing about a town. Pubs in the Republic of Ireland sell neither Carling nor Bell's, so Coors and Jameson stand in and they are ranked in euros.

## MCP server

`spoons mcp` serves a GraphQL API over MCP, as two tools in the house style of its sibling servers:
`spoons_schema` returns the SDL ([`crates/spoons-mcp/schema.graphql`](crates/spoons-mcp/schema.graphql)), and
`spoons` runs a query or mutation, with variables as a JSON string. For Claude Code:

```console
$ claude mcp add spoons --env SPOONS_EMAIL=you@example.com -- spoons mcp
```

`SPOONS_EMAIL` is where the receipt goes.

The graph starts at `pubs(query:, near:)` or `pub(ref:)`. A pub has its hours, wait times, ale list and tables, and
`menu(table:)`: everything the pub sells to that table, in kinds (cask ales, lager, craft and world beer, stout,
cider, wine, spirits, cocktails, alcohol-free, soft and hot drinks, food), with each product's sizes and prices,
ABV, units, calories, taste notes and colour read from the menu's prose, the guest ales picked out (they are named
"Brewery - Beer" and never mention ale), and why anything that can't be ordered can't. A garden can be a sales area
with its own menu and prices, which is why the table matters.

Each MCP session has a basket on the server (`basketAdd`, `basketSet`, `basketRemove`, `basketClear`,
`basketFromQuote` for "same again", `setTable`), so the model and anything rendering the results share one. `quote`
prices the basket, or explicit items, at the till for a table and changes nothing. Nothing is quoted without a table,
and the server never picks one. `pay`, `cancelOrder` and `sendDuplicate` are PREVIEW then CONFIRM: PREVIEW changes
nothing and returns a token bound to its arguments, CONFIRM spends it, and the server instructions tell the model
to confirm only on the user's explicit yes. `order(quoteId:, waitSecs:)` follows a payment.

"Four pints of Abbot to table 4":

```graphql
{ pub(ref: 254) { menu(table: 4, search: "abbot") { products { nodes { id name portions { name price { formatted } } } } } } }

mutation { basketAdd(pub: 254, table: 4, productId: "10000000315", portion: "Pint", quantity: 4) { basket { summary } }
           quote { quote { id summary total { formatted } differsFromMenu } } }

mutation { pay(quoteId: "d44e019b7a8e", action: PREVIEW) { preview confirmationToken } }
# shown to the user; on their yes:
mutation { pay(quoteId: "d44e019b7a8e", action: CONFIRM, confirmationToken: "…") { status paymentPage card question next } }
```

`spoons mcp --http 0.0.0.0:8080` serves the same tools over streamable HTTP at `/mcp`, which is how the image runs.
Each request's `X-Spoons-Email` header gives its receipt address, and `SPOONS_EMAIL` is ignored: the server is
shared, so a gateway in front injects each user's own, and `pay` returns the payment page as a link instead of
opening it. The server authenticates nobody, so it belongs behind a proxy that does. `--state` says where quotes are
kept; the image keeps them in the container, where losing them is safe: a quote that no longer exists can't be paid,
and a payment nobody pays lapses. Baskets and confirmation tokens are in memory and go with a restart, which only
means building the basket or previewing again. The payment page is BNP Paribas Axepta's, where the card is entered
and the bank approves it.

Over HTTP, `pay` can enter a saved card itself. The card comes in four headers, `X-Spoons-Card-Number`,
`X-Spoons-Card-Expiry` (`MM/YY`), `X-Spoons-Card-CVC` and `X-Spoons-Card-Name`, which a gateway in front keeps as
the user's secrets; the tools never take card details as arguments, so a card never passes through the assistant.
Wetherspoon's keeps no cards, so this is the only way to pay without the page. `pay` posts the card as the page's own
script does, follows the 3-D Secure redirects and self-submitting pages, then reads the result from the payment API.
Axepta's 3-D Secure 2 page runs the bank's device check in a hidden frame through script; without a browser, `pay`
skips it the way that script does when the bank is slow, by telling Axepta the check failed. Axepta then either
redirects to its result, when the bank lets the payment through without the card holder, or asks for a challenge in
the banking app, which needs the payment page: `pay` reports that the card was sent and not to pay again.
The card is entered at most once per payment, and recorded as entered before it is sent. When the page never took
it, the payment is still pending and `pay` hands out the page as usual; when something failed after the card was
sent, it reports the payment's status and does not offer the page, since the card may have been charged.

Menus are read from the API once per sales area per London day and kept in memory, so a session of quotes does not
fetch a pub's whole menu (several megabytes) on every call. A quote's prices and total still come from the till, which
checks the basket live; when the till refuses a basket, or a product is missing, that menu is fetched again on the next
call.

Quotes and payments are kept under `$XDG_STATE_HOME/spoons` (`~/.local/state/spoons`), with a device id the payment
API needs to cancel a payment it started. The server is careful about the cases that cost money:

- **A payment is never prepared twice for one quote.** `pay` holds a lock on the quote across processes and returns
  the existing payment when called again.
- **A request to prepare or send a payment is never retried.** One that gets no clear answer may still have made the
  order, so the quote is marked unconfirmed and `pay` refuses it until the user has checked for a receipt or charge.
- **Nothing cancels on its own.** Cancelling while the bank is authorising a card can leave a charge with no order,
  so `cancelOrder` only acts before a card is entered, or on an order held as a duplicate. The payment page cancels
  itself if abandoned.
- **Repeat orders are the user's call.** The payment API flags a basket that matches a recent order to the same
  table, before payment (`pay` asks) or after it (the order is held, and `sendDuplicate` or `cancelOrder` settles
  it).

Products that need a choice made, such as a spirit's mixer or a meal's side, can't be quoted yet; the menu says
which (`orderable`, `whyNot`).

## Things worth knowing about the API

- **Prices are set per pub, in zones.** The same pint of Carling was £3.49 in Lewisham and £2.99 in Wigan on
  24 September 2026, and pubs in one zone share identical prices.
- **Some prices are chain-wide placeholders.** A drink priced identically at pubs in different zones is not
  zone-priced; compare before trusting one pub's figure.
- **Spirits are priced with a mixer.** Every mixer option costs nothing, including "No mixer", so the price is the
  measure as the bar sells it.
- **The firewall only admits requests that look like the order-and-pay site.** A request with no User-Agent, or one
  naming this project, is refused with a 403, so the client sends the site's own headers.
- **IDs are numbers or strings depending on the app version sent**, and a pub's specials menu has the ID `specials`.
- **The six pubs in the Republic of Ireland price in euros.**
- **Not every pub is under `jdw`.** Two dozen are Wetherspoon bars run for someone else: on Haven holiday parks,
  university campuses and Papa's sites. Their routes use their own franchise, and the Haven bars have no menus in
  the app at all.
- **A size's label can carry its calories** (`250ml glass (185 kcal, 3.0 units)`), and which label a pub gets has
  changed from one menu version to the next, so sizes are matched by how the label starts.
