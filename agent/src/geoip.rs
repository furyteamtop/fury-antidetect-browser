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

/// Where a check asks for the exit IP, and where it reads geo from.
#[derive(Debug, Clone, PartialEq)]
pub struct Checker {
    pub endpoint: String,
    pub db: Option<String>,
    /// The database is the one Fury fetched (geodb.rs), not one the operator
    /// named: a lookup that fails falls back to the checker quietly, and a
    /// country that disagrees with the checker's is reported.
    pub bundled: bool,
}

/// The checker for a proxy: what its field says, else `FURY_IP_CHECK`, else
/// ipinfo.io for the address with the fetched database for the rest.
pub fn checker(field: Option<&str>) -> Checker {
    let named = field
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("FURY_IP_CHECK").ok().filter(|s| !s.trim().is_empty()));
    match named {
        Some(v) => {
            let (endpoint, db) = split_checker(&v);
            Checker { endpoint, db, bundled: false }
        }
        None => Checker {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            db: crate::geodb::ready().map(|p| p.to_string_lossy().into_owned()),
            bundled: true,
        },
    }
}

/// What one check says about an exit, the database and the checker merged.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Exit {
    pub ip: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
    /// "lat,lng", already validated.
    pub location: Option<String>,
    /// The checker's own country when the fetched database places the exit
    /// elsewhere: ipinfo.io's answer, which is where the server physically
    /// is. Shown beside the country, never used for the profile.
    pub checker_country: Option<String>,
    /// Why the database was not used, when it was asked and failed.
    pub db_error: Option<String>,
}

/// The exit IP out of a checker's answer, or why there is none (issue 39).
/// A rate-limit page, an error JSON or a captive portal comes back through a
/// working proxy with no `ip`, and used to read as a pass with an empty
/// verdict: a checkmark that names no exit. The reason is what the operator
/// can act on: the HTTP status, the checker's own error text, or the start
/// of whatever it sent.
pub fn exit_ip(status: u16, text: &str) -> Result<(serde_json::Value, String), NoIp> {
    let body: serde_json::Value = serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
    if let Some(ip) = body.get("ip").and_then(|v| v.as_str()).map(str::trim).filter(|ip| !ip.is_empty()) {
        let ip = ip.to_string();
        return Ok((body, ip));
    }
    let said = body
        .get("error")
        .map(|e| match e {
            serde_json::Value::String(s) => s.clone(),
            other => ["title", "message", "detail"]
                .iter()
                .find_map(|k| other.get(*k).and_then(|v| v.as_str()).map(str::to_string))
                .unwrap_or_else(|| other.to_string()),
        })
        .or_else(|| body.get("message").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| {
            let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut cut: String = flat.chars().take(80).collect();
            if flat.chars().count() > 80 {
                cut.push('…');
            }
            if cut.is_empty() { "an empty answer".to_string() } else { format!("\"{cut}\"") }
        });
    Err(NoIp { status, said: said.chars().take(120).collect() })
}

/// A checker answer with no IP in it: the HTTP status and the checker's own
/// words, short enough to sit inside the interface's sentence.
#[derive(Debug)]
pub struct NoIp {
    pub status: u16,
    pub said: String,
}

impl NoIp {
    /// "HTTP 429: Rate limit exceeded", or just the words on a 200.
    pub fn short(&self) -> String {
        if (200..300).contains(&self.status) {
            self.said.clone()
        } else {
            format!("HTTP {}: {}", self.status, self.said)
        }
    }
}

impl std::fmt::Display for NoIp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The IP checker answered without an IP address ({}).", self.short())
    }
}

impl std::error::Error for NoIp {}

/// Merge the checker's JSON with the database: per field the database wins
/// when it has an answer, the checker is the fallback.
pub fn overlay(body: &serde_json::Value, checker: &Checker) -> Exit {
    let ip = body.get("ip").and_then(|v| v.as_str());
    let geo = match (&checker.db, ip) {
        (Some(db), Some(ip)) => Some(lookup(db, ip)),
        _ => None,
    };
    merge(body, geo, checker.bundled)
}

fn merge(body: &serde_json::Value, geo: Option<anyhow::Result<Geo>>, bundled: bool) -> Exit {
    let s = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let valid = |v: Option<String>| v.filter(|l| crate::ipc::parse_location(l).is_some());
    let mut out = Exit {
        ip: s("ip"),
        country: s("country"),
        city: s("city"),
        timezone: s("timezone"),
        location: valid(s("loc")),
        ..Default::default()
    };
    match geo {
        Some(Ok(geo)) => {
            if bundled && geo.country.is_some() && out.country.is_some() && geo.country != out.country {
                out.checker_country = out.country.clone();
            }
            out.country = geo.country.or(out.country);
            out.city = geo.city.or(out.city);
            // The zone and the position follow the database together: a
            // Mexican exit with New York's clock is what this is for.
            if geo.timezone.is_some() || geo.location.is_some() {
                out.timezone = geo.timezone.or(out.timezone);
                out.location = valid(geo.location).or(out.location);
            }
        }
        Some(Err(e)) => out.db_error = Some(format!("{e:#}")),
        None => {}
    }
    out
}

/// Country / city / timezone / "lat,lng" for one exit IP from a local file.
#[derive(Debug, Default)]
pub struct Geo {
    pub country: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
    pub location: Option<String>,
}

type Mapped = std::sync::Arc<maxminddb::Reader<maxminddb::Mmap>>;

/// Mapped, not read: DB-IP City Lite is 127 MB, and reading it per lookup (or
/// holding a copy) costs that much memory in a process that runs all day.
/// Kept per path and reopened when the file changes; a file replaced under a
/// mapping keeps its old inode until the mapping goes.
fn open(path: &std::path::Path) -> anyhow::Result<Mapped> {
    static CACHE: std::sync::Mutex<Vec<(std::path::PathBuf, std::time::SystemTime, Mapped)>> =
        std::sync::Mutex::new(Vec::new());
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| anyhow::anyhow!("{:?} does not open as an MMDB database: {e}", path.display().to_string()))?;
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((_, _, r)) = cache.iter().find(|(p, m, _)| p == path && *m == modified) {
        return Ok(r.clone());
    }
    // SAFETY (of the mapping): the files mapped are ours or the operator's,
    // and ours are replaced by rename, never written in place.
    let reader = unsafe { maxminddb::Reader::open_mmap(path) }
        .map_err(|e| anyhow::anyhow!("{:?} does not open as an MMDB database: {e}", path.display().to_string()))?;
    let reader = std::sync::Arc::new(reader);
    cache.retain(|(p, _, _)| p != path);
    cache.push((path.to_path_buf(), modified, reader.clone()));
    Ok(reader)
}

/// The IANA zone at a point, from boundary data compiled into the agent.
/// None at sea (the data answers "Etc/GMT+2" there, which no browser in a
/// real place reports).
pub fn zone_at(lat: f64, lng: f64) -> Option<String> {
    static FINDER: std::sync::OnceLock<tzf_rs::DefaultFinder> = std::sync::OnceLock::new();
    let name = FINDER.get_or_init(tzf_rs::DefaultFinder::new).get_tz_name(lng, lat);
    (!name.is_empty() && name.contains('/') && !name.starts_with("Etc/")).then(|| name.to_string())
}

/// Refused with the path in the message rather than failing later: a path
/// the operator pasted that does not open must read as one, not as silence.
pub fn lookup(db: &str, ip: &str) -> anyhow::Result<Geo> {
    let reader = open(std::path::Path::new(db))?;
    let addr: std::net::IpAddr = ip
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("{ip:?} is not an IP address"))?;
    let result = reader.lookup(addr).map_err(|e| anyhow::anyhow!("{e}"))?;
    // City carries country + city + timezone + coordinates; a Country file
    // decodes as one with only the country set, which is still an answer.
    if let Ok(Some(city)) = result.decode::<maxminddb::geoip2::City>() {
        let (lat, lng) = (city.location.latitude, city.location.longitude);
        let out = Geo {
            country: city.country.iso_code.map(str::to_string),
            city: city.city.names.english.map(str::to_string),
            // DB-IP Lite has coordinates and no zone: the zone is where
            // the coordinates are.
            timezone: city
                .location
                .time_zone
                .map(str::to_string)
                .or_else(|| lat.zip(lng).and_then(|(lat, lng)| zone_at(lat, lng))),
            location: match (lat, lng) {
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
mod exit_ip_tests {
    use super::exit_ip;

    #[test]
    fn an_answer_with_an_ip_passes() {
        let (_, ip) = exit_ip(200, r#"{"ip":"38.110.1.246","country":"US"}"#).unwrap();
        assert_eq!(ip, "38.110.1.246");
    }

    #[test]
    fn a_rate_limit_names_the_status_and_the_checkers_words() {
        let e = exit_ip(429, r#"{"status":429,"error":{"title":"Rate limit exceeded","message":"Upgrade"}}"#).unwrap_err();
        let e = e.to_string();
        assert!(e.contains("HTTP 429"), "{e}");
        assert!(e.contains("Rate limit exceeded"), "{e}");
    }

    #[test]
    fn a_page_that_is_not_json_is_quoted_short() {
        let page = format!("<html><body>{}</body></html>", "x".repeat(500));
        let e = exit_ip(200, &page).unwrap_err().to_string();
        assert!(e.starts_with("The IP checker answered without an IP address (\"<html>"), "{e}");
        assert!(e.chars().count() < 200, "{e}");
    }

    #[test]
    fn an_empty_or_blank_ip_is_no_ip() {
        assert_eq!(exit_ip(200, "").unwrap_err().short(), "an empty answer");
        assert!(exit_ip(200, r#"{"ip":"  "}"#).is_err());
        assert!(exit_ip(200, r#"{"ip":null,"error":"bogon"}"#).unwrap_err().short().contains("bogon"));
    }
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

    /// The tester's case (18.11.2026): ipinfo.io puts the server in New York,
    /// the databases sites read put the address in Mexico. The profile gets
    /// Mexico, its zone and its position; ipinfo's US is kept to be shown.
    #[test]
    fn the_database_wins_and_a_disagreeing_checker_is_kept_to_be_shown() {
        let body = serde_json::json!({
            "ip": "187.13.122.237", "country": "US", "city": "New York City",
            "timezone": "America/New_York", "loc": "40.7143,-74.0060", "org": "AS62240 Clouvider"
        });
        let geo = || Geo {
            country: Some("MX".into()),
            city: Some("Mexico City".into()),
            timezone: Some("America/Mexico_City".into()),
            location: Some("19.4326,-99.1332".into()),
        };
        let e = merge(&body, Some(Ok(geo())), true);
        assert_eq!(e.country.as_deref(), Some("MX"));
        assert_eq!(e.timezone.as_deref(), Some("America/Mexico_City"));
        assert_eq!(e.location.as_deref(), Some("19.4326,-99.1332"));
        assert_eq!(e.checker_country.as_deref(), Some("US"));
        assert_eq!(e.ip.as_deref(), Some("187.13.122.237"));
        // A file the operator named: same answer, nothing to point out.
        assert_eq!(merge(&body, Some(Ok(geo())), false).checker_country, None);
        // Agreeing sources: nothing to point out either.
        let us = Geo { country: Some("US".into()), ..geo() };
        assert_eq!(merge(&body, Some(Ok(us)), true).checker_country, None);
    }

    #[test]
    fn without_the_database_the_checker_answers_alone() {
        let body = serde_json::json!({"ip": "1.2.3.4", "country": "DE", "timezone": "Europe/Berlin", "loc": "52.5,13.4"});
        let e = merge(&body, None, true);
        assert_eq!((e.country.as_deref(), e.timezone.as_deref(), e.location.as_deref()), (Some("DE"), Some("Europe/Berlin"), Some("52.5,13.4")));
        let e = merge(&body, Some(Err(anyhow::anyhow!("no record"))), true);
        assert_eq!(e.country.as_deref(), Some("DE"));
        assert_eq!(e.db_error.as_deref(), Some("no record"));
        // A database with only a country leaves the checker's zone alone.
        let e = merge(&body, Some(Ok(Geo { country: Some("AT".into()), ..Default::default() })), true);
        assert_eq!((e.country.as_deref(), e.timezone.as_deref()), (Some("AT"), Some("Europe/Berlin")));
    }

    #[test]
    fn the_zone_is_where_the_coordinates_are() {
        assert_eq!(zone_at(19.4326, -99.1332).as_deref(), Some("America/Mexico_City"));
        assert_eq!(zone_at(35.6174, 139.748).as_deref(), Some("Asia/Tokyo"));
        assert_eq!(zone_at(43.6532, -79.3832).as_deref(), Some("America/Toronto"));
        assert_eq!(zone_at(25.67, -100.31).as_deref(), Some("America/Monterrey"));
        // Mid-Atlantic: no place, no zone.
        assert_eq!(zone_at(30.0, -30.0), None);
    }

    /// Against the real DB-IP file, when one is given:
    /// FURY_TEST_GEO_DB=/path/dbip-city-lite.mmdb cargo test -p fury-agent geoip
    #[test]
    fn the_real_database_places_the_testers_proxies_where_sites_do() {
        let Ok(db) = std::env::var("FURY_TEST_GEO_DB") else {
            eprintln!("SKIPPED: FURY_TEST_GEO_DB not set");
            return;
        };
        for (ip, country, zone) in [
            ("187.13.122.237", "MX", "America/Mexico_City"),
            ("5.183.129.239", "CA", "America/Toronto"),
            ("130.49.15.95", "US", "America/New_York"),
            ("8.8.8.8", "US", "America/Los_Angeles"),
        ] {
            let g = lookup(&db, ip).unwrap();
            assert_eq!((g.country.as_deref(), g.timezone.as_deref()), (Some(country), Some(zone)), "{ip}");
            assert!(g.location.is_some(), "{ip}");
        }
    }

    #[test]
    fn a_file_that_does_not_open_is_named() {
        let err = lookup("/definitely/not/there.mmdb", "8.8.8.8").unwrap_err();
        assert!(err.to_string().contains("not/there.mmdb"), "{err}");
    }
}
