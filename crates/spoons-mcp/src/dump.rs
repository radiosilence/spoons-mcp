use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::de::DeserializeOwned;
use spoons_api_client::{Client, MenuSummary, Venue, VenueSummary, paths};

use crate::Menus;

/// Bodies are kept exactly as sent, so anything the types here don't model is still in the dump. Menus are gzipped:
/// one pub's drinks menu is 5 MB of JSON, most of it the same options repeated, and 330 KB compressed.
pub async fn run(client: &Client, out: &Path, menus: Menus, only: &[u64]) -> Result<()> {
    fs::create_dir_all(out.join("pubs"))?;
    let venues: Vec<VenueSummary> = data(&fetch(client, &paths::venues(), &out.join("venues.json"), true).await?)?;
    let venues: Vec<_> = venues.into_iter().filter(|venue| only.is_empty() || only.contains(&venue.venue_ref)).collect();
    let mut failed = Vec::new();
    for (i, venue) in venues.iter().enumerate() {
        tracing::info!("{}/{} {}", i + 1, venues.len(), venue.name);
        if let Err(error) = dump_pub(client, &out.join("pubs").join(venue.venue_ref.to_string()), &venue.franchise, venue.venue_ref, menus).await {
            tracing::warn!("{} ({}): {error:#}", venue.name, venue.venue_ref);
            failed.push(venue.venue_ref);
        }
    }
    if !failed.is_empty() {
        tracing::warn!("{} pubs failed; run again to retry them: {failed:?}", failed.len());
    }
    Ok(())
}

async fn dump_pub(client: &Client, dir: &Path, franchise: &str, venue: u64, menus: Menus) -> Result<()> {
    fs::create_dir_all(dir)?;
    let detail: Venue = data(&fetch(client, &paths::venue(franchise, venue), &dir.join("venue.json"), false).await?)?;
    // Not every pub has these: a pub not taking orders reports no wait, and the holiday-park bars have no guest ales.
    // Either answers with an error, which is no reason to skip the menus.
    for (route, file) in [(paths::ales(franchise, venue), "ales.json"), (paths::wait_times(franchise, venue), "wait-times.json")] {
        if let Err(error) = fetch(client, &route, &dir.join(file), false).await {
            tracing::debug!("no {file}: {error:#}");
        }
    }
    for area in &detail.sales_areas {
        let list: Vec<MenuSummary> = data(&fetch(client, &paths::menus(franchise, venue, area.id), &dir.join(format!("menus-{}.json", area.id)), false).await?)?;
        for menu in list.iter().filter(|menu| wanted(menu, menus)) {
            let path = dir.join(format!("menu-{}-{}.json.gz", area.id, menu.id));
            fetch(client, &paths::menu(franchise, venue, area.id, &menu.id), &path, false).await?;
        }
    }
    Ok(())
}

fn wanted(menu: &MenuSummary, menus: Menus) -> bool {
    match menus {
        Menus::All => true,
        Menus::Drinks => ["Drinks", "Real Ales"].iter().any(|name| menu.name.eq_ignore_ascii_case(name)),
    }
}

/// The saved body if there is one, otherwise fetched and saved. The venue list is always fetched fresh, since it's
/// what says which pubs exist.
async fn fetch(client: &Client, route: &str, path: &Path, fresh: bool) -> Result<Vec<u8>> {
    let gzipped = path.extension().is_some_and(|extension| extension == "gz");
    if !fresh && path.exists() {
        return read(path, gzipped);
    }
    let body = client.raw(route).await?;
    let partial = PathBuf::from(format!("{}.partial", path.display()));
    if gzipped {
        let mut encoder = GzEncoder::new(fs::File::create(&partial)?, Compression::default());
        encoder.write_all(&body)?;
        encoder.finish()?;
    } else {
        fs::write(&partial, &body)?;
    }
    fs::rename(&partial, path)?;
    Ok(body)
}

pub fn read(path: &Path, gzipped: bool) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    if gzipped {
        GzDecoder::new(fs::File::open(path)?).read_to_end(&mut body)?;
    } else {
        fs::File::open(path)?.read_to_end(&mut body)?;
    }
    Ok(body)
}

/// The `data` of an API envelope.
pub fn data<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    #[derive(serde::Deserialize)]
    struct Envelope<T> {
        data: T,
    }
    Ok(serde_json::from_slice::<Envelope<T>>(body).context("not the shape expected")?.data)
}
