# spoons-mcp

Wetherspoon's pubs, menus, prices, real ales and wait times, from the API behind its order-and-pay app.

The API is unofficial and undocumented. It is REST, behind a proxy at `ca.jdw-apps.net`, and every route except the
venue list sits under the franchise (`/jdw/…`). The bearer token is built into the order-and-pay site's public
JavaScript bundle, so the client reads it from there at start-up instead of anyone keeping a copy. Only read routes
are used; the basket, order and payment routes are not.

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
being the cheapest tenth and F the dearest twentieth. Pubs in the Republic of Ireland are ranked separately, in euros.

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
