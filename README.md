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

`spoons mcp` serves tools over stdio for ordering to a table: `find_pubs`, `menu`, `quote`, `pay`, `order_status`,
`cancel_order` and `send_duplicate`. For Claude Code:

```console
$ claude mcp add spoons --env SPOONS_EMAIL=you@example.com -- spoons mcp
```

`SPOONS_EMAIL` is where the receipt goes.

`spoons mcp --http 0.0.0.0:8080` serves the same tools over streamable HTTP at `/mcp`, which is how the image runs.
Each request's `X-Spoons-Email` header gives its receipt address, and `SPOONS_EMAIL` is ignored: the server is
shared, so a gateway in front injects each user's own,
and `pay` returns the payment page as a link instead of opening it. The server authenticates nobody, so it belongs
behind a proxy that does. `--state` says where quotes are kept; the image keeps them in the container, where losing
them is safe: a quote that no longer exists can't be paid, and a payment nobody pays lapses. Ordering is two steps so the user sees the price before anything starts:
`quote` prices the basket at the pub's till and changes nothing, and `pay` prepares the card payment and opens the
payment page (BNP Paribas Axepta) in the browser, where the card is entered and the bank approves it. The tools never
take card details.

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
  so `cancel_order` only acts before a card is entered, or on an order held as a duplicate. The payment page cancels
  itself if abandoned.
- **Repeat orders are the user's call.** The payment API flags a basket that matches a recent order to the same
  table, before payment (`pay` asks) or after it (the order is held, and `send_duplicate` or `cancel_order` settles
  it).

Products that need a choice made, such as a spirit's mixer, can't be quoted yet.

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
