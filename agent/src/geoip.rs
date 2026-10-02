// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! A local MMDB as the proxy exit checker (issue #4).
//!
//! The checker field takes a URL or a path to a local MMDB file. A path
//! means: ask the default checker for the exit IP only — the address is
//! something only the far end can report — and resolve country, city,
//! timezone and coordinates from the file. Per field the file wins when it
//! has an answer and the checker is the fallback; `org` always comes from
//! the checker.

/// The exit IP goes out through the proxy either way; only the geo source
/// changes, so a path still asks here and not nowhere.
pub const DEFAULT_ENDPOINT: &str = "https://ipinfo.io/json";

/// Split the resolved checker value into the URL to ask and the local file
/// to resolve geo from, if it is one. Anything without a scheme is a path:
/// a pasted `ipinfo.io/json` was the 01.10.2026 loop — reqwest fails it at
/// send time as a bare "builder error" blamed on the proxy — so it reads as
/// a path that does not open rather than as a broken proxy.
pub fn split_checker(resolved: &str) -> (String, Option<String>) {
    let trimmed = resolved.trim().to_string();
    if trimmed.contains("://") {
        (trimmed, None)
    } else {
        (DEFAULT_ENDPOINT.to_string(), Some(trimmed))
    }
}

/// Country / city / timezone / "lat,lng" for one exit IP from a local file.
#[derive(Debug, Default)]
pub struct Geo {
    pub country: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
    pub location: Option<String>,
}

/// Refused with the path in the message rather than failing later: a path
/// the operator pasted that does not open must read as one, not as silence.
pub fn lookup(db: &str, ip: &str) -> anyhow::Result<Geo> {
    let path = std::path::Path::new(db);
    let reader = maxminddb::Reader::open_readfile(path)
        .map_err(|e| anyhow::anyhow!("{db:?} does not open as an MMDB database: {e}"))?;
    let addr: std::net::IpAddr = ip
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("{ip:?} is not an IP address"))?;
    let result = reader.lookup(addr).map_err(|e| anyhow::anyhow!("{e}"))?;
    // City carries country + city + timezone + coordinates; a Country file
    // decodes as one with only the country set, which is still an answer.
    if let Ok(Some(city)) = result.decode::<maxminddb::geoip2::City>() {
        let out = Geo {
            country: city.country.iso_code.map(str::to_string),
            city: city.city.names.english.map(str::to_string),
            timezone: city.location.time_zone.map(str::to_string),
            location: match (city.location.latitude, city.location.longitude) {
                (Some(lat), Some(lng)) => Some(format!("{lat},{lng}")),
                _ => None,
            },
        };
        if out.country.is_some() || out.city.is_some() || out.timezone.is_some() || out.location.is_some() {
            return Ok(out);
        }
    }
    if let Ok(Some(country)) = result.decode::<maxminddb::geoip2::Country>() {
        if let Some(iso) = country.country.iso_code {
            return Ok(Geo { country: Some(iso.to_string()), ..Default::default() });
        }
    }
    anyhow::bail!("{db:?} has no record for {ip}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_stays_a_url_and_a_path_becomes_the_default_plus_that_path() {
        assert_eq!(split_checker("https://ipinfo.io/json"), ("https://ipinfo.io/json".to_string(), None));
        assert_eq!(
            split_checker("/Users/me/city.mmdb"),
            (DEFAULT_ENDPOINT.to_string(), Some("/Users/me/city.mmdb".to_string()))
        );
        // The 01.10.2026 loop: no scheme is a path, never a proxy fault.
        let (endpoint, db) = split_checker("ipinfo.io/json");
        assert_eq!(endpoint, DEFAULT_ENDPOINT);
        assert!(db.is_some());
    }

    #[test]
    fn a_file_that_does_not_open_is_named() {
        let err = lookup("/definitely/not/there.mmdb", "8.8.8.8").unwrap_err();
        assert!(err.to_string().contains("not/there.mmdb"), "{err}");
    }
}
